#![cfg(test)]

extern crate std;

use super::*;
use soroban_sdk::{
    testutils::{Address as _, Events as _, Ledger as _, MockAuth, MockAuthInvoke},
    token::Client as TokenClient,
    token::StellarAssetClient,
    Address, Env, IntoVal, String, Symbol, Vec,
};

#[test]
fn test_allowance_handshake_and_pull() {
    let env = Env::default();

    // In unit tests, we mock the wallet signatures (mock_all_auths)
    // so we can test the logic without a real frontend connecting.
    env.mock_all_auths();

    // Generate dummy addresses for our actors
    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token_admin = Address::generate(&env);

    // Deploy a mock testnet token (represents our USDC with 7 decimals)
    let token_address = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = TokenClient::new(&env, &token_address);
    let token_admin_client = StellarAssetClient::new(&env, &token_address);

    // Mint 100 USDC to the user to start
    token_admin_client.mint(&user, &100_0000000);

    // Register our Orbit Contract
    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    // ------------------------------------------------------------------------
    // SCENARIO 1: THE HANDSHAKE
    // ------------------------------------------------------------------------
    let amount = 29_0000000; // 29 USDC (with 7 zeroes for precision)
    let interval = 30 * 24 * 60 * 60; // 30 days in seconds

    // User approves the contract to spend the recurring charge
    token_client.approve(&user, &orbit_client.address, &amount, &2000000);

    // User creates the vault (approving the 29 USDC recurring charge)
    orbit_client.create_vault(&user, &merchant, &token_address, &amount, &interval);

    // ------------------------------------------------------------------------
    // SCENARIO 2: THE PULL
    // ------------------------------------------------------------------------
    // Merchant executes the billing
    orbit_client.pull_funds(&user, &merchant);

    // Verify the math! The merchant should have exactly 29 USDC.
    assert_eq!(token_client.balance(&merchant), 29_0000000);
    // The user should have 100 - 29 = 71 USDC remaining.
    assert_eq!(token_client.balance(&user), 71_0000000);
}

// ----------------------------------------------------------------------------
// ISSUE #1: INPUT VALIDATION IN CREATE_VAULT
// ----------------------------------------------------------------------------

#[test]
#[should_panic]
fn test_create_vault_fails_when_amount_zero() {
    let env = Env::default();
    env.mock_all_auths();

    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token = Address::generate(&env);

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    orbit_client.create_vault(&user, &merchant, &token, &0, &86400);
}

#[test]
#[should_panic]
fn test_create_vault_fails_when_amount_negative() {
    let env = Env::default();
    env.mock_all_auths();

    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token = Address::generate(&env);

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    orbit_client.create_vault(&user, &merchant, &token, &-1, &86400);
}

#[test]
#[should_panic]
fn test_create_vault_fails_when_interval_zero() {
    let env = Env::default();
    env.mock_all_auths();

    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token = Address::generate(&env);

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    orbit_client.create_vault(&user, &merchant, &token, &29_0000000, &0);
}

// ----------------------------------------------------------------------------
// ISSUE #2: EXPANDED UNIT TEST COVERAGE
// ----------------------------------------------------------------------------

#[test]
#[should_panic]
fn test_pull_before_interval_has_elapsed_fails() {
    let env = Env::default();
    env.mock_all_auths();

    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token_admin = Address::generate(&env);

    let token_address = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = TokenClient::new(&env, &token_address);
    let token_admin_client = StellarAssetClient::new(&env, &token_address);
    token_admin_client.mint(&user, &100_0000000);

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    let amount = 29_0000000;
    let interval = 86400; // 1 day

    token_client.approve(&user, &orbit_client.address, &100_0000000, &2000000);
    orbit_client.create_vault(&user, &merchant, &token_address, &amount, &interval);

    // Initial pull at timestamp 1000
    env.ledger().set_timestamp(1000);
    orbit_client.pull_funds(&user, &merchant);

    // Advance timestamp by less than interval (1000 + 40000 < 1000 + 86400)
    env.ledger().set_timestamp(1000 + 40000);

    // Fails because interval hasn't elapsed yet
    orbit_client.pull_funds(&user, &merchant);
}

#[test]
fn test_second_pull_succeeds_once_interval_has_elapsed() {
    let env = Env::default();
    env.mock_all_auths();

    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token_admin = Address::generate(&env);

    let token_address = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = TokenClient::new(&env, &token_address);
    let token_admin_client = StellarAssetClient::new(&env, &token_address);
    token_admin_client.mint(&user, &100_0000000);

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    let amount = 29_0000000;
    let interval = 86400; // 1 day

    token_client.approve(&user, &orbit_client.address, &100_0000000, &2000000);
    orbit_client.create_vault(&user, &merchant, &token_address, &amount, &interval);

    // First pull at timestamp 1000
    env.ledger().set_timestamp(1000);
    orbit_client.pull_funds(&user, &merchant);
    assert_eq!(token_client.balance(&merchant), 29_0000000);
    assert_eq!(token_client.balance(&user), 71_0000000);

    // Advance timestamp by exactly the interval (1000 + 86400)
    env.ledger().set_timestamp(1000 + 86400);
    orbit_client.pull_funds(&user, &merchant);

    // Second pull succeeds
    assert_eq!(token_client.balance(&merchant), 58_0000000);
    assert_eq!(token_client.balance(&user), 42_0000000);
}

#[test]
#[should_panic]
fn test_pull_on_vault_that_does_not_exist_fails() {
    let env = Env::default();
    env.mock_all_auths();

    let user = Address::generate(&env);
    let merchant = Address::generate(&env);

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    // Pulling on non-existent vault should fail with "Vault does not exist"
    orbit_client.pull_funds(&user, &merchant);
}

#[test]
#[should_panic]
fn test_pull_authorized_by_different_merchant_fails() {
    let env = Env::default();
    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let other_merchant = Address::generate(&env);
    let token_admin = Address::generate(&env);

    let token_address = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = TokenClient::new(&env, &token_address);
    let token_admin_client = StellarAssetClient::new(&env, &token_address);
    token_admin_client.mint(&user, &100_0000000);

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    // Create vault with mock_all_auths first
    env.mock_all_auths();
    token_client.approve(&user, &orbit_client.address, &29_0000000, &2000000);
    orbit_client.create_vault(&user, &merchant, &token_address, &29_0000000, &86400);

    // Now mock auth ONLY for other_merchant
    orbit_client.mock_auths(&[MockAuth {
        address: &other_merchant,
        invoke: &MockAuthInvoke {
            contract: &orbit_client.address,
            fn_name: "pull_funds",
            args: (&user, &merchant).into_val(&env),
            sub_invokes: &[],
        },
    }]);

    // Pulling with merchant while only other_merchant authorized should fail
    orbit_client.pull_funds(&user, &merchant);
}

#[test]
fn test_create_vault_overwrites_existing_terms_and_resets_last_pull_timestamp() {
    let env = Env::default();
    env.mock_all_auths();

    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token_admin = Address::generate(&env);

    let token_address = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = TokenClient::new(&env, &token_address);
    let token_admin_client = StellarAssetClient::new(&env, &token_address);
    token_admin_client.mint(&user, &100_0000000);

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    token_client.approve(&user, &orbit_client.address, &100_0000000, &2000000);

    // Create initial vault: 29 USDC, 86400s interval
    orbit_client.create_vault(&user, &merchant, &token_address, &29_0000000, &86400);

    // Pull at t = 1000
    env.ledger().set_timestamp(1000);
    orbit_client.pull_funds(&user, &merchant);
    assert_eq!(token_client.balance(&merchant), 29_0000000);

    // Overwrite the vault terms with 40 USDC and 172800s interval
    orbit_client.create_vault(&user, &merchant, &token_address, &40_0000000, &172800);

    // Check persistent storage to verify overwritten terms and reset last_pull_timestamp = 0
    let key = vault_key(&user, &merchant);
    let vault_before_pull: VaultData =
        env.as_contract(&contract_id, || load_vault(&env, &key).unwrap());
    assert_eq!(vault_before_pull.amount_per_interval, 40_0000000);
    assert_eq!(vault_before_pull.interval_seconds, 172800);
    assert_eq!(vault_before_pull.last_pull_timestamp, 0);

    // Because last_pull_timestamp was reset to 0, pulling immediately at t = 1000 succeeds with new amount!
    orbit_client.pull_funds(&user, &merchant);
    assert_eq!(token_client.balance(&merchant), 69_0000000); // 29 + 40
    assert_eq!(token_client.balance(&user), 31_0000000); // 100 - 69
}

#[test]
fn test_batch_disburse_pays_every_recipient_correct_amount() {
    let env = Env::default();
    env.mock_all_auths();

    let sender = Address::generate(&env);
    let recipient_1 = Address::generate(&env);
    let recipient_2 = Address::generate(&env);
    let recipient_3 = Address::generate(&env);
    let token_admin = Address::generate(&env);

    let token_address = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = TokenClient::new(&env, &token_address);
    let token_admin_client = StellarAssetClient::new(&env, &token_address);

    token_admin_client.mint(&sender, &100_0000000);

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    token_client.approve(&sender, &orbit_client.address, &60_0000000, &2000000);

    let mut splits = Vec::new(&env);
    splits.push_back(PaymentSplit {
        recipient: recipient_1.clone(),
        amount: 10_0000000,
    });
    splits.push_back(PaymentSplit {
        recipient: recipient_2.clone(),
        amount: 20_0000000,
    });
    splits.push_back(PaymentSplit {
        recipient: recipient_3.clone(),
        amount: 30_0000000,
    });

    orbit_client.batch_disburse(&sender, &token_address, &splits);

    assert_eq!(token_client.balance(&recipient_1), 10_0000000);
    assert_eq!(token_client.balance(&recipient_2), 20_0000000);
    assert_eq!(token_client.balance(&recipient_3), 30_0000000);
    assert_eq!(token_client.balance(&sender), 40_0000000);
}

#[test]
fn test_batch_disburse_reverts_fully_when_transfer_fails() {
    let env = Env::default();
    env.mock_all_auths();

    let sender = Address::generate(&env);
    let recipient_1 = Address::generate(&env);
    let recipient_2 = Address::generate(&env);
    let token_admin = Address::generate(&env);

    let token_address = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = TokenClient::new(&env, &token_address);
    let token_admin_client = StellarAssetClient::new(&env, &token_address);

    token_admin_client.mint(&sender, &100_0000000);

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    // Only approve 30 USDC, but splits total 40 USDC
    token_client.approve(&sender, &orbit_client.address, &30_0000000, &2000000);

    let mut splits = Vec::new(&env);
    splits.push_back(PaymentSplit {
        recipient: recipient_1.clone(),
        amount: 20_0000000,
    });
    splits.push_back(PaymentSplit {
        recipient: recipient_2.clone(),
        amount: 20_0000000,
    });

    let res = orbit_client.try_batch_disburse(&sender, &token_address, &splits);
    assert!(res.is_err());

    // Verify atomic revert: recipient 1 must have 0, recipient 2 must have 0, sender must still have 100
    assert_eq!(token_client.balance(&recipient_1), 0);
    assert_eq!(token_client.balance(&recipient_2), 0);
    assert_eq!(token_client.balance(&sender), 100_0000000);
}

// ----------------------------------------------------------------------------
// ISSUE #21: READ-ONLY VAULT LOOKUP
// ----------------------------------------------------------------------------

#[test]
fn test_get_vault_returns_none_when_vault_does_not_exist() {
    let env = Env::default();

    let user = Address::generate(&env);
    let merchant = Address::generate(&env);

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    let vault = orbit_client.get_vault(&user, &merchant);

    assert_eq!(vault, None);
}

#[test]
fn test_get_vault_returns_existing_vault_and_latest_pull_timestamp() {
    let env = Env::default();
    env.mock_all_auths();

    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token_admin = Address::generate(&env);

    let token_address = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = TokenClient::new(&env, &token_address);
    let token_admin_client = StellarAssetClient::new(&env, &token_address);

    token_admin_client.mint(&user, &100_0000000);

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    let amount = 20_0000000;
    let interval = 86400;

    token_client.approve(&user, &orbit_client.address, &100_0000000, &2000000);

    orbit_client.create_vault(&user, &merchant, &token_address, &amount, &interval);

    let created_vault = orbit_client
        .get_vault(&user, &merchant)
        .expect("vault should exist");

    assert_eq!(created_vault.token, token_address);
    assert_eq!(created_vault.amount_per_interval, amount);
    assert_eq!(created_vault.interval_seconds, interval);
    assert_eq!(created_vault.last_pull_timestamp, 0);

    env.ledger().set_timestamp(1000);
    orbit_client.pull_funds(&user, &merchant);

    let after_first_pull = orbit_client
        .get_vault(&user, &merchant)
        .expect("vault should still exist");

    assert_eq!(after_first_pull.last_pull_timestamp, 1000);

    env.ledger().set_timestamp(1000 + interval);
    orbit_client.pull_funds(&user, &merchant);

    let after_second_pull = orbit_client
        .get_vault(&user, &merchant)
        .expect("vault should still exist");

    assert_eq!(after_second_pull.last_pull_timestamp, 1000 + interval);
}

// ----------------------------------------------------------------------------
// ISSUE #23: VAULT & CONTRACT INSTANCE STORAGE TTL EXTENSION
// ----------------------------------------------------------------------------

#[test]
fn test_calculate_ttl_params_logic() {
    // 1. Short interval (e.g. 1 day = 86,400s -> ~17,280 ledgers)
    // 17,280 + 120,960 = 138,240, which is below MIN_TTL_LEDGERS (518,400).
    // It should be floored at MIN_TTL_LEDGERS.
    let (short_threshold, short_extend_to) = calculate_ttl_params(86_400);
    assert_eq!(short_extend_to, MIN_TTL_LEDGERS);
    assert_eq!(short_threshold, TTL_THRESHOLD_LEDGERS);

    // 2. Long interval (e.g. 1 year = 31,536,000s -> 6,307,200 ledgers)
    // 6,307,200 + 120,960 = 6,428,160 ledgers.
    let yearly_seconds = 365 * 24 * 60 * 60;
    let (yearly_threshold, yearly_extend_to) = calculate_ttl_params(yearly_seconds);
    assert_eq!(
        yearly_extend_to,
        (yearly_seconds / SECONDS_PER_LEDGER) as u32 + TTL_MARGIN_LEDGERS
    );
    assert_eq!(yearly_threshold, TTL_THRESHOLD_LEDGERS);
}

#[test]
fn test_vault_ttl_extended_and_survives_past_default_ttl() {
    let env = Env::default();
    env.mock_all_auths();

    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token_admin = Address::generate(&env);

    let token_address = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = TokenClient::new(&env, &token_address);
    let token_admin_client = StellarAssetClient::new(&env, &token_address);

    token_admin_client.mint(&user, &100_0000000);

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    let amount = 29_0000000;
    let interval = 30 * 24 * 60 * 60; // 30 days = 2,592,000s (~518,400 ledgers)

    token_client.approve(&user, &orbit_client.address, &100_0000000, &2000000);

    // 1. Create vault: extends persistent & instance TTL
    orbit_client.create_vault(&user, &merchant, &token_address, &amount, &interval);

    let initial_seq = env.ledger().sequence();
    let advanced_seq = initial_seq + 10_000; // Past default 4,096 TTL
    let advanced_time = interval + 100; // Past interval

    env.ledger().set_sequence_number(advanced_seq);
    env.ledger().set_timestamp(advanced_time);

    // 2. The vault must still exist in storage and pull_funds must succeed!
    orbit_client.pull_funds(&user, &merchant);

    assert_eq!(token_client.balance(&merchant), amount);
    assert_eq!(token_client.balance(&user), 100_0000000 - amount);

    // 3. Verify that pull_funds also extended TTL, allowing a second pull further in time
    let second_advanced_seq = advanced_seq + 10_000;
    let second_advanced_time = advanced_time + interval + 100;

    env.ledger().set_sequence_number(second_advanced_seq);
    env.ledger().set_timestamp(second_advanced_time);

    orbit_client.pull_funds(&user, &merchant);

    assert_eq!(token_client.balance(&merchant), amount * 2);
    assert_eq!(token_client.balance(&user), 100_0000000 - (amount * 2));
}

// ----------------------------------------------------------------------------
// ISSUE #22: VALIDATE SPLITS IN BATCH_DISBURSE
// ----------------------------------------------------------------------------

#[test]
fn test_batch_disburse_fails_when_splits_empty() {
    let env = Env::default();
    env.mock_all_auths();

    let sender = Address::generate(&env);
    let token = Address::generate(&env);

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    let splits = Vec::new(&env);
    let res = orbit_client.try_batch_disburse(&sender, &token, &splits);
    assert!(res.is_err());
}

#[test]
fn test_batch_disburse_fails_when_split_amount_zero_and_no_recipient_paid() {
    let env = Env::default();
    env.mock_all_auths();

    let sender = Address::generate(&env);
    let recipient_1 = Address::generate(&env);
    let recipient_2 = Address::generate(&env);
    let token_admin = Address::generate(&env);

    let token_address = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = TokenClient::new(&env, &token_address);
    let token_admin_client = StellarAssetClient::new(&env, &token_address);

    token_admin_client.mint(&sender, &100_0000000);

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    token_client.approve(&sender, &orbit_client.address, &100_0000000, &2000000);

    let mut splits = Vec::new(&env);
    // First split is positive, second split is zero
    splits.push_back(PaymentSplit {
        recipient: recipient_1.clone(),
        amount: 25_0000000,
    });
    splits.push_back(PaymentSplit {
        recipient: recipient_2.clone(),
        amount: 0,
    });

    let res = orbit_client.try_batch_disburse(&sender, &token_address, &splits);
    assert!(res.is_err());

    // Upfront validation ensures zero transfers took place
    assert_eq!(token_client.balance(&recipient_1), 0);
    assert_eq!(token_client.balance(&recipient_2), 0);
    assert_eq!(token_client.balance(&sender), 100_0000000);
}

#[test]
fn test_batch_disburse_fails_when_split_amount_negative_and_no_recipient_paid() {
    let env = Env::default();
    env.mock_all_auths();

    let sender = Address::generate(&env);
    let recipient_1 = Address::generate(&env);
    let recipient_2 = Address::generate(&env);
    let token_admin = Address::generate(&env);

    let token_address = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = TokenClient::new(&env, &token_address);
    let token_admin_client = StellarAssetClient::new(&env, &token_address);

    token_admin_client.mint(&sender, &100_0000000);

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    token_client.approve(&sender, &orbit_client.address, &100_0000000, &2000000);

    let mut splits = Vec::new(&env);
    // First split is positive, second split is negative
    splits.push_back(PaymentSplit {
        recipient: recipient_1.clone(),
        amount: 30_0000000,
    });
    splits.push_back(PaymentSplit {
        recipient: recipient_2.clone(),
        amount: -10_0000000,
    });

    let res = orbit_client.try_batch_disburse(&sender, &token_address, &splits);
    assert!(res.is_err());

    // Upfront validation ensures zero transfers took place
    assert_eq!(token_client.balance(&recipient_1), 0);
    assert_eq!(token_client.balance(&recipient_2), 0);
    assert_eq!(token_client.balance(&sender), 100_0000000);
}

// ----------------------------------------------------------------------------
// ISSUE #20: EMIT EVENTS FOR CONTRACT ENTRYPOINTS
// ----------------------------------------------------------------------------

#[test]
fn test_contract_events_published_for_all_entrypoints() {
    let env = Env::default();
    env.mock_all_auths();

    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token_admin = Address::generate(&env);

    let token_address = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = TokenClient::new(&env, &token_address);
    let token_admin_client = StellarAssetClient::new(&env, &token_address);

    token_admin_client.mint(&user, &100_0000000);

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    let amount = 29_0000000;
    let interval = 86400;

    token_client.approve(&user, &orbit_client.address, &100_0000000, &2000000);

    // 1. Verify create_vault event
    orbit_client.create_vault(&user, &merchant, &token_address, &amount, &interval);

    let create_topics = soroban_sdk::vec![
        &env,
        Symbol::new(&env, "vault_created").to_val(),
        user.to_val(),
        merchant.to_val(),
    ];
    let create_data = (token_address.clone(), amount, interval, 0u64).into_val(&env);
    let expected_create = soroban_sdk::vec![
        &env,
        (orbit_client.address.clone(), create_topics, create_data),
    ];
    assert_eq!(env.events().all(), expected_create);

    // 2. Verify pull_funds event
    env.ledger().set_timestamp(5000);
    orbit_client.pull_funds(&user, &merchant);

    let pull_topics = soroban_sdk::vec![
        &env,
        Symbol::new(&env, "funds_pulled").to_val(),
        user.clone().to_val(),
        merchant.clone().to_val(),
    ];
    let pull_data = (amount, 5000u64).into_val(&env);
    let expected_pull =
        soroban_sdk::vec![&env, (orbit_client.address.clone(), pull_topics, pull_data),];
    assert_eq!(
        env.events().all().filter_by_contract(&orbit_client.address),
        expected_pull
    );

    // 3. Verify batch_disburse event
    let sender = Address::generate(&env);
    let rec1 = Address::generate(&env);
    let rec2 = Address::generate(&env);

    token_admin_client.mint(&sender, &50_0000000);
    token_client.approve(&sender, &orbit_client.address, &50_0000000, &2000000);

    let mut splits = Vec::new(&env);
    splits.push_back(PaymentSplit {
        recipient: rec1,
        amount: 15_0000000,
    });
    splits.push_back(PaymentSplit {
        recipient: rec2,
        amount: 25_0000000,
    });

    orbit_client.batch_disburse(&sender, &token_address, &splits);

    let disburse_topics = soroban_sdk::vec![
        &env,
        Symbol::new(&env, "batch_disbursed").to_val(),
        sender.clone().to_val(),
    ];
    let disburse_data = (token_address.clone(), 2u32, 40_0000000i128).into_val(&env);
    let expected_disburse = soroban_sdk::vec![
        &env,
        (orbit_client.address.clone(), disburse_topics, disburse_data),
    ];
    assert_eq!(
        env.events().all().filter_by_contract(&orbit_client.address),
        expected_disburse
    );
}

// ----------------------------------------------------------------------------
// ISSUE #44: CANCEL VAULT
// ----------------------------------------------------------------------------

#[test]
fn test_cancel_vault_removes_vault_and_blocks_pull() {
    let env = Env::default();
    env.mock_all_auths();

    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token_admin = Address::generate(&env);

    let token_address = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = TokenClient::new(&env, &token_address);
    let token_admin_client = StellarAssetClient::new(&env, &token_address);
    token_admin_client.mint(&user, &100_0000000);

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    token_client.approve(&user, &orbit_client.address, &100_0000000, &2000000);
    orbit_client.create_vault(&user, &merchant, &token_address, &29_0000000, &86400);

    orbit_client.cancel_vault(&user, &merchant);

    assert_eq!(orbit_client.get_vault(&user, &merchant), None);

    // The allowance is still in place, but with no vault the pull must fail
    let res = orbit_client.try_pull_funds(&user, &merchant);
    assert!(res.is_err());
    assert_eq!(token_client.balance(&merchant), 0);
    assert_eq!(token_client.balance(&user), 100_0000000);
}

#[test]
fn test_cancel_vault_fails_when_vault_does_not_exist() {
    let env = Env::default();
    env.mock_all_auths();

    let user = Address::generate(&env);
    let merchant = Address::generate(&env);

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    let res = orbit_client.try_cancel_vault(&user, &merchant);
    assert!(res.is_err());
}

#[test]
fn test_cancel_vault_authorized_by_merchant_fails() {
    let env = Env::default();
    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token = Address::generate(&env);

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    env.mock_all_auths();
    orbit_client.create_vault(&user, &merchant, &token, &29_0000000, &86400);

    // Now mock auth ONLY for the merchant
    let res = orbit_client
        .mock_auths(&[MockAuth {
            address: &merchant,
            invoke: &MockAuthInvoke {
                contract: &orbit_client.address,
                fn_name: "cancel_vault",
                args: (&user, &merchant).into_val(&env),
                sub_invokes: &[],
            },
        }])
        .try_cancel_vault(&user, &merchant);
    assert!(res.is_err());

    // The vault is untouched
    assert!(orbit_client.get_vault(&user, &merchant).is_some());
}

#[test]
fn test_create_vault_works_again_after_cancel() {
    let env = Env::default();
    env.mock_all_auths();

    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token_admin = Address::generate(&env);

    let token_address = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = TokenClient::new(&env, &token_address);
    let token_admin_client = StellarAssetClient::new(&env, &token_address);
    token_admin_client.mint(&user, &100_0000000);

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    token_client.approve(&user, &orbit_client.address, &100_0000000, &2000000);

    orbit_client.create_vault(&user, &merchant, &token_address, &29_0000000, &86400);
    orbit_client.cancel_vault(&user, &merchant);
    orbit_client.create_vault(&user, &merchant, &token_address, &40_0000000, &172800);

    let vault = orbit_client
        .get_vault(&user, &merchant)
        .expect("vault should exist");
    assert_eq!(vault.amount_per_interval, 40_0000000);
    assert_eq!(vault.interval_seconds, 172800);
    assert_eq!(vault.last_pull_timestamp, 0);

    orbit_client.pull_funds(&user, &merchant);
    assert_eq!(token_client.balance(&merchant), 40_0000000);
}

#[test]
fn test_cancel_vault_event_published() {
    let env = Env::default();
    env.mock_all_auths();

    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token = Address::generate(&env);

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    orbit_client.create_vault(&user, &merchant, &token, &29_0000000, &86400);
    orbit_client.cancel_vault(&user, &merchant);

    let cancel_topics = soroban_sdk::vec![
        &env,
        Symbol::new(&env, "vault_cancelled").to_val(),
        user.to_val(),
        merchant.to_val(),
    ];
    let expected_cancel = soroban_sdk::vec![
        &env,
        (
            orbit_client.address.clone(),
            cancel_topics,
            ().into_val(&env)
        ),
    ];
    assert_eq!(env.events().all(), expected_cancel);
}

// ----------------------------------------------------------------------------
// ISSUE #65: MULTI-SIG / THRESHOLD AUTHORIZATION FOR BATCH PAYROLL
// ----------------------------------------------------------------------------

struct PayrollSetup<'a> {
    env: Env,
    orbit_client: OrbitContractClient<'a>,
    token_client: TokenClient<'a>,
    token_address: Address,
    sender: Address,
    recipient_1: Address,
    recipient_2: Address,
    approvers: Vec<Address>,
    splits: Vec<PaymentSplit>,
}

/// Sender with 100 USDC, a 60 USDC allowance to the contract, three approvers and
/// two splits totalling 30 USDC. All auths are mocked.
fn setup_payroll<'a>() -> PayrollSetup<'a> {
    let env = Env::default();
    env.mock_all_auths();

    let sender = Address::generate(&env);
    let recipient_1 = Address::generate(&env);
    let recipient_2 = Address::generate(&env);
    let token_admin = Address::generate(&env);

    let token_address = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = TokenClient::new(&env, &token_address);
    StellarAssetClient::new(&env, &token_address).mint(&sender, &100_0000000);

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);
    token_client.approve(&sender, &orbit_client.address, &60_0000000, &2000000);

    let approvers = soroban_sdk::vec![
        &env,
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    ];
    let mut splits = Vec::new(&env);
    splits.push_back(PaymentSplit {
        recipient: recipient_1.clone(),
        amount: 10_0000000,
    });
    splits.push_back(PaymentSplit {
        recipient: recipient_2.clone(),
        amount: 20_0000000,
    });

    PayrollSetup {
        env,
        orbit_client,
        token_client,
        token_address,
        sender,
        recipient_1,
        recipient_2,
        approvers,
        splits,
    }
}

impl PayrollSetup<'_> {
    fn propose(&self, required_approvals: u32) -> u64 {
        self.orbit_client.propose_batch_disburse(
            &self.sender,
            &self.token_address,
            &self.splits,
            &required_approvals,
            &self.approvers,
        )
    }

    fn approver(&self, index: u32) -> Address {
        self.approvers.get(index).unwrap()
    }

    fn assert_nobody_paid(&self) {
        assert_eq!(self.token_client.balance(&self.recipient_1), 0);
        assert_eq!(self.token_client.balance(&self.recipient_2), 0);
        assert_eq!(self.token_client.balance(&self.sender), 100_0000000);
    }
}

#[test]
fn test_failed_pull_enters_grace_and_settled_pull_restores_active() {
    let env = Env::default();
    env.mock_all_auths();
    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(token_admin)
        .address();
    let token_client = TokenClient::new(&env, &token);
    StellarAssetClient::new(&env, &token).mint(&user, &100);
    let contract_id = env.register(OrbitContract, ());
    let client = OrbitContractClient::new(&env, &contract_id);
    token_client.approve(&user, &client.address, &100, &2000);
    client.create_vault_with_grace_period(&user, &merchant, &token, &10, &100, &50);

    env.ledger().set_timestamp(101);
    client.record_failed_pull(&merchant, &user);
    assert_eq!(
        client.get_vault(&user, &merchant).unwrap().payment_status,
        PAYMENT_GRACE_PERIOD
    );

    client.pull_funds(&user, &merchant);
    assert_eq!(
        client.get_vault(&user, &merchant).unwrap().payment_status,
        PAYMENT_ACTIVE
    );
    assert_eq!(token_client.balance(&merchant), 10);
}

#[test]
fn test_failed_pull_defaults_and_defaulted_pull_is_blocked() {
    let env = Env::default();
    env.mock_all_auths();
    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token = Address::generate(&env);
    let contract_id = env.register(OrbitContract, ());
    let client = OrbitContractClient::new(&env, &contract_id);
    client.create_vault_with_grace_period(&user, &merchant, &token, &10, &100, &50);
    env.ledger().set_timestamp(150);
    client.record_failed_pull(&merchant, &user);
    assert_eq!(
        client.get_vault(&user, &merchant).unwrap().payment_status,
        PAYMENT_DEFAULTED
    );
    assert!(client.try_pull_funds(&user, &merchant).is_err());
    client.merchant_cancel_vault(&merchant, &user);
    assert_eq!(client.get_vault(&user, &merchant), None);
}

#[test]
fn test_merchant_cancel_and_refund() {
    let env = Env::default();
    env.mock_all_auths();
    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(token_admin)
        .address();
    let token_client = TokenClient::new(&env, &token);
    StellarAssetClient::new(&env, &token).mint(&merchant, &100);
    let contract_id = env.register(OrbitContract, ());
    let client = OrbitContractClient::new(&env, &contract_id);
    client.create_vault(&user, &merchant, &token, &10, &100);
    client.merchant_cancel_vault(&merchant, &user);
    assert_eq!(client.get_vault(&user, &merchant), None);
    token_client.approve(&merchant, &client.address, &25, &2000);
    client.refund_subscriber(&merchant, &user, &token, &25);
    assert_eq!(token_client.balance(&user), 25);
}

#[test]
fn test_update_vault_terms_preserves_last_pull_and_uses_new_amount() {
    let env = Env::default();
    env.mock_all_auths();
    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(token_admin)
        .address();
    let token_client = TokenClient::new(&env, &token);
    StellarAssetClient::new(&env, &token).mint(&user, &100);
    let contract_id = env.register(OrbitContract, ());
    let client = OrbitContractClient::new(&env, &contract_id);
    token_client.approve(&user, &client.address, &100, &2000);
    client.create_vault(&user, &merchant, &token, &10, &100);
    env.ledger().set_timestamp(10);
    client.pull_funds(&user, &merchant);
    client.update_vault_terms(&user, &merchant, &25, &200);
    let vault = client.get_vault(&user, &merchant).unwrap();
    assert_eq!(vault.last_pull_timestamp, 10);
    assert_eq!(vault.amount_per_interval, 25);
    env.ledger().set_timestamp(210);
    client.pull_funds(&user, &merchant);
    assert_eq!(token_client.balance(&merchant), 35);
}

#[test]
fn test_stream_withdraws_linearly_and_at_completion() {
    let env = Env::default();
    env.mock_all_auths();
    let sender = Address::generate(&env);
    let recipient = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(token_admin)
        .address();
    let token_client = TokenClient::new(&env, &token);
    StellarAssetClient::new(&env, &token).mint(&sender, &100);
    let contract_id = env.register(OrbitContract, ());
    let client = OrbitContractClient::new(&env, &contract_id);
    token_client.approve(&sender, &client.address, &100, &2000);
    let stream_id = client.create_stream(&sender, &recipient, &token, &100, &100, &200);
    assert_eq!(token_client.balance(&sender), 0);
    env.ledger().set_timestamp(150);
    client.withdraw_from_stream(&stream_id, &25);
    assert_eq!(token_client.balance(&recipient), 25);
    env.ledger().set_timestamp(200);
    client.withdraw_from_stream(&stream_id, &75);
    assert_eq!(token_client.balance(&recipient), 100);
}

#[test]
fn test_stream_early_cancel_splits_vested_and_unvested_amounts() {
    let env = Env::default();
    env.mock_all_auths();
    let sender = Address::generate(&env);
    let recipient = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(token_admin)
        .address();
    let token_client = TokenClient::new(&env, &token);
    StellarAssetClient::new(&env, &token).mint(&sender, &100);
    let contract_id = env.register(OrbitContract, ());
    let client = OrbitContractClient::new(&env, &contract_id);
    token_client.approve(&sender, &client.address, &100, &2000);
    let stream_id = client.create_stream(&sender, &recipient, &token, &100, &100, &200);
    env.ledger().set_timestamp(120);
    client.cancel_stream(&stream_id);
    assert_eq!(token_client.balance(&recipient), 20);
    assert_eq!(token_client.balance(&sender), 80);
}

// ----------------------------------------------------------------------------
// ISSUE #93: SUBSCRIPTION PAUSE & RESUME
// ----------------------------------------------------------------------------

#[test]
fn test_pause_blocks_pull_and_resume_restores_billing() {
    let env = Env::default();
    env.mock_all_auths();

    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token_admin = Address::generate(&env);

    let token_address = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = TokenClient::new(&env, &token_address);
    StellarAssetClient::new(&env, &token_address).mint(&user, &100_0000000);

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);
    token_client.approve(&user, &orbit_client.address, &100_0000000, &2000000);

    let amount = 29_0000000;
    let interval = 86400;
    orbit_client.create_vault(&user, &merchant, &token_address, &amount, &interval);

    env.ledger().set_timestamp(1000);
    orbit_client.pull_funds(&user, &merchant);
    assert_eq!(token_client.balance(&merchant), amount);

    orbit_client.pause_vault(&user, &merchant);
    assert!(orbit_client.get_vault(&user, &merchant).unwrap().is_paused);

    env.ledger().set_timestamp(1000 + interval);
    assert!(orbit_client.try_pull_funds(&user, &merchant).is_err());
    assert_eq!(token_client.balance(&merchant), amount);

    orbit_client.resume_vault(&user, &merchant);
    let resumed = orbit_client.get_vault(&user, &merchant).unwrap();
    assert!(!resumed.is_paused);
    assert_eq!(resumed.last_pull_timestamp, 1000 + interval);
    assert!(orbit_client.try_pull_funds(&user, &merchant).is_err());

    env.ledger().set_timestamp(1000 + 2 * interval);
    orbit_client.pull_funds(&user, &merchant);
    assert_eq!(token_client.balance(&merchant), amount * 2);
    assert_eq!(token_client.balance(&user), 100_0000000 - amount * 2);
}

#[test]
fn test_propose_payroll_stores_pending_proposal_with_sequential_ids() {
    let s = setup_payroll();

    let first = s.propose(2);
    let second = s.propose(3);
    assert_eq!(first, 0);
    assert_eq!(second, 1);

    let proposal = s
        .orbit_client
        .get_payroll_proposal(&first)
        .expect("proposal should exist");
    assert_eq!(
        proposal,
        PayrollProposal {
            id: 0,
            sender: s.sender.clone(),
            token: s.token_address.clone(),
            splits: s.splits.clone(),
            approvers: s.approvers.clone(),
            approvals: Vec::new(&s.env),
            required_approvals: 2,
            executed: false,
        }
    );
    assert_eq!(s.orbit_client.get_payroll_proposal(&2), None);

    // Proposing moves no funds
    s.assert_nobody_paid();
}

#[test]
fn test_payroll_executes_after_threshold_of_distinct_approvals() {
    let s = setup_payroll();
    let id = s.propose(2);

    s.orbit_client.approve_payroll_proposal(&s.approver(0), &id);
    s.orbit_client.approve_payroll_proposal(&s.approver(2), &id);
    s.orbit_client.execute_payroll_proposal(&id);

    // events().all() only covers the most recent invocation, so check it before any other call
    let executed_topics = soroban_sdk::vec![
        &s.env,
        Symbol::new(&s.env, "payroll_executed").to_val(),
        s.sender.to_val(),
        id.into_val(&s.env),
    ];
    let executed_data = (s.token_address.clone(), 2u32, 30_0000000i128).into_val(&s.env);
    assert_eq!(
        s.env
            .events()
            .all()
            .filter_by_contract(&s.orbit_client.address),
        soroban_sdk::vec![
            &s.env,
            (
                s.orbit_client.address.clone(),
                executed_topics,
                executed_data
            ),
        ]
    );

    assert_eq!(s.token_client.balance(&s.recipient_1), 10_0000000);
    assert_eq!(s.token_client.balance(&s.recipient_2), 20_0000000);
    assert_eq!(s.token_client.balance(&s.sender), 70_0000000);

    let proposal = s.orbit_client.get_payroll_proposal(&id).unwrap();
    assert!(proposal.executed);
    assert_eq!(
        proposal.approvals,
        soroban_sdk::vec![&s.env, s.approver(0), s.approver(2)]
    );
}

#[test]
fn test_payroll_execution_fails_below_threshold() {
    let s = setup_payroll();
    let id = s.propose(2);

    assert!(s.orbit_client.try_execute_payroll_proposal(&id).is_err());

    s.orbit_client.approve_payroll_proposal(&s.approver(1), &id);
    assert!(s.orbit_client.try_execute_payroll_proposal(&id).is_err());

    s.assert_nobody_paid();
    assert!(!s.orbit_client.get_payroll_proposal(&id).unwrap().executed);
}

#[test]
fn test_payroll_counts_each_approver_once() {
    let s = setup_payroll();
    let id = s.propose(2);

    s.orbit_client.approve_payroll_proposal(&s.approver(0), &id);
    let res = s
        .orbit_client
        .try_approve_payroll_proposal(&s.approver(0), &id);
    assert!(res.is_err());

    // One distinct approval is still short of a 2-of-3 threshold
    assert_eq!(
        s.orbit_client
            .get_payroll_proposal(&id)
            .unwrap()
            .approvals
            .len(),
        1
    );
    assert!(s.orbit_client.try_execute_payroll_proposal(&id).is_err());
    s.assert_nobody_paid();
}

#[test]
fn test_payroll_rejects_approval_from_address_not_in_approver_list() {
    let s = setup_payroll();
    let id = s.propose(1);

    let outsider = Address::generate(&s.env);
    assert!(s
        .orbit_client
        .try_approve_payroll_proposal(&outsider, &id)
        .is_err());

    // The sender is not an approver unless listed
    assert!(s
        .orbit_client
        .try_approve_payroll_proposal(&s.sender, &id)
        .is_err());

    assert!(s.orbit_client.try_execute_payroll_proposal(&id).is_err());
    s.assert_nobody_paid();
}

#[test]
fn test_payroll_approval_requires_the_approvers_own_signature() {
    let s = setup_payroll();
    let id = s.propose(1);
    let approver = s.approver(0);
    let other_approver = s.approver(1);

    // Another listed approver signing cannot approve on `approver`'s behalf
    let res = s
        .orbit_client
        .mock_auths(&[MockAuth {
            address: &other_approver,
            invoke: &MockAuthInvoke {
                contract: &s.orbit_client.address,
                fn_name: "approve_payroll_proposal",
                args: (&approver, id).into_val(&s.env),
                sub_invokes: &[],
            },
        }])
        .try_approve_payroll_proposal(&approver, &id);
    assert!(res.is_err());

    let res = s
        .orbit_client
        .mock_auths(&[MockAuth {
            address: &approver,
            invoke: &MockAuthInvoke {
                contract: &s.orbit_client.address,
                fn_name: "approve_payroll_proposal",
                args: (&approver, id).into_val(&s.env),
                sub_invokes: &[],
            },
        }])
        .try_approve_payroll_proposal(&approver, &id);
    assert!(res.is_ok());
    assert_eq!(
        s.orbit_client.get_payroll_proposal(&id).unwrap().approvals,
        soroban_sdk::vec![&s.env, approver]
    );
}

#[test]
fn test_propose_payroll_requires_sender_signature() {
    let s = setup_payroll();
    let impostor = Address::generate(&s.env);
    let required_approvals: u32 = 1;

    let res = s
        .orbit_client
        .mock_auths(&[MockAuth {
            address: &impostor,
            invoke: &MockAuthInvoke {
                contract: &s.orbit_client.address,
                fn_name: "propose_batch_disburse",
                args: (
                    &s.sender,
                    &s.token_address,
                    &s.splits,
                    required_approvals,
                    &s.approvers,
                )
                    .into_val(&s.env),
                sub_invokes: &[],
            },
        }])
        .try_propose_batch_disburse(
            &s.sender,
            &s.token_address,
            &s.splits,
            &required_approvals,
            &s.approvers,
        );
    assert!(res.is_err());
    assert_eq!(s.orbit_client.get_payroll_proposal(&0), None);
}

#[test]
fn test_payroll_cannot_be_executed_twice() {
    let s = setup_payroll();
    let id = s.propose(1);

    s.orbit_client.approve_payroll_proposal(&s.approver(0), &id);
    s.orbit_client.execute_payroll_proposal(&id);
    assert_eq!(s.token_client.balance(&s.sender), 70_0000000);

    // Replay: the remaining 30 USDC allowance would cover a second run, but it must not happen
    assert!(s.orbit_client.try_execute_payroll_proposal(&id).is_err());
    assert_eq!(s.token_client.balance(&s.recipient_1), 10_0000000);
    assert_eq!(s.token_client.balance(&s.recipient_2), 20_0000000);
    assert_eq!(s.token_client.balance(&s.sender), 70_0000000);

    // Nor can it collect further approvals
    assert!(s
        .orbit_client
        .try_approve_payroll_proposal(&s.approver(1), &id)
        .is_err());
}

#[test]
fn test_payroll_execution_reverts_atomically_and_can_be_retried() {
    let s = setup_payroll();
    let id = s.propose(1);
    s.orbit_client.approve_payroll_proposal(&s.approver(0), &id);

    // Allowance too small for the 30 USDC payroll
    s.token_client
        .approve(&s.sender, &s.orbit_client.address, &15_0000000, &2000000);
    assert!(s.orbit_client.try_execute_payroll_proposal(&id).is_err());

    // Nothing moved and the executed flag was rolled back with the failed transfers
    s.assert_nobody_paid();
    assert!(!s.orbit_client.get_payroll_proposal(&id).unwrap().executed);

    s.token_client
        .approve(&s.sender, &s.orbit_client.address, &30_0000000, &2000000);
    s.orbit_client.execute_payroll_proposal(&id);
    assert_eq!(s.token_client.balance(&s.recipient_2), 20_0000000);
    assert!(s.orbit_client.get_payroll_proposal(&id).unwrap().executed);
}

#[test]
fn test_propose_payroll_rejects_invalid_threshold() {
    let s = setup_payroll();

    // Zero approvals required
    assert!(s
        .orbit_client
        .try_propose_batch_disburse(&s.sender, &s.token_address, &s.splits, &0, &s.approvers)
        .is_err());
    // More approvals required than there are approvers
    assert!(s
        .orbit_client
        .try_propose_batch_disburse(&s.sender, &s.token_address, &s.splits, &4, &s.approvers)
        .is_err());
    // A threshold equal to the approver count is allowed
    assert!(s
        .orbit_client
        .try_propose_batch_disburse(&s.sender, &s.token_address, &s.splits, &3, &s.approvers)
        .is_ok());
}

#[test]
fn test_propose_payroll_rejects_invalid_approver_lists() {
    let s = setup_payroll();
    let approver = s.approver(0);

    // Empty list
    assert!(s
        .orbit_client
        .try_propose_batch_disburse(
            &s.sender,
            &s.token_address,
            &s.splits,
            &1,
            &Vec::new(&s.env)
        )
        .is_err());

    // A duplicated approver could otherwise satisfy a 2-of-2 threshold alone
    let duplicated = soroban_sdk::vec![&s.env, approver.clone(), approver];
    assert!(s
        .orbit_client
        .try_propose_batch_disburse(&s.sender, &s.token_address, &s.splits, &2, &duplicated)
        .is_err());

    // Over the approver cap
    let mut too_many = Vec::new(&s.env);
    for _ in 0..(MAX_PAYROLL_APPROVERS + 1) {
        too_many.push_back(Address::generate(&s.env));
    }
    assert!(s
        .orbit_client
        .try_propose_batch_disburse(&s.sender, &s.token_address, &s.splits, &1, &too_many)
        .is_err());
}

#[test]
fn test_propose_payroll_rejects_invalid_splits() {
    let s = setup_payroll();

    assert!(s
        .orbit_client
        .try_propose_batch_disburse(
            &s.sender,
            &s.token_address,
            &Vec::new(&s.env),
            &1,
            &s.approvers
        )
        .is_err());

    let mut zero_split = s.splits.clone();
    zero_split.push_back(PaymentSplit {
        recipient: Address::generate(&s.env),
        amount: 0,
    });
    assert!(s
        .orbit_client
        .try_propose_batch_disburse(&s.sender, &s.token_address, &zero_split, &1, &s.approvers)
        .is_err());
}

#[test]
fn test_payroll_actions_fail_for_unknown_proposal() {
    let s = setup_payroll();

    assert!(s
        .orbit_client
        .try_approve_payroll_proposal(&s.approver(0), &42)
        .is_err());
    assert!(s.orbit_client.try_execute_payroll_proposal(&42).is_err());
}

#[test]
fn test_payroll_proposal_and_approval_events_published() {
    let s = setup_payroll();
    let id = s.propose(2);

    let proposed_topics = soroban_sdk::vec![
        &s.env,
        Symbol::new(&s.env, "payroll_proposed").to_val(),
        s.sender.to_val(),
        id.into_val(&s.env),
    ];
    let proposed_data = (s.token_address.clone(), 2u32, 3u32, 30_0000000i128).into_val(&s.env);
    assert_eq!(
        s.env
            .events()
            .all()
            .filter_by_contract(&s.orbit_client.address),
        soroban_sdk::vec![
            &s.env,
            (
                s.orbit_client.address.clone(),
                proposed_topics,
                proposed_data
            ),
        ]
    );

    s.orbit_client.approve_payroll_proposal(&s.approver(1), &id);

    let approved_topics = soroban_sdk::vec![
        &s.env,
        Symbol::new(&s.env, "payroll_approved").to_val(),
        s.approver(1).to_val(),
        id.into_val(&s.env),
    ];
    let approved_data = (1u32, 2u32).into_val(&s.env);
    assert_eq!(
        s.env
            .events()
            .all()
            .filter_by_contract(&s.orbit_client.address),
        soroban_sdk::vec![
            &s.env,
            (
                s.orbit_client.address.clone(),
                approved_topics,
                approved_data
            ),
        ]
    );
}

#[test]
fn test_failed_pull_enters_grace_and_settled_pull_restores_active() {
    let env = Env::default();
    env.mock_all_auths();
    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let token = env.register_stellar_asset_contract_v2(token_admin).address();
    let token_client = TokenClient::new(&env, &token);
    StellarAssetClient::new(&env, &token).mint(&user, &100);
    let contract_id = env.register(OrbitContract, ());
    let client = OrbitContractClient::new(&env, &contract_id);
    token_client.approve(&user, &client.address, &100, &2000);
    client.create_vault_with_grace_period(&user, &merchant, &token, &10, &100, &50);

    env.ledger().set_timestamp(101);
    client.record_failed_pull(&merchant, &user);
    assert_eq!(client.get_vault(&user, &merchant).unwrap().payment_status, PAYMENT_GRACE_PERIOD);

    client.pull_funds(&user, &merchant);
    assert_eq!(client.get_vault(&user, &merchant).unwrap().payment_status, PAYMENT_ACTIVE);
    assert_eq!(token_client.balance(&merchant), 10);
}

#[test]
fn test_failed_pull_defaults_and_defaulted_pull_is_blocked() {
    let env = Env::default();
    env.mock_all_auths();
    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token = Address::generate(&env);
    let contract_id = env.register(OrbitContract, ());
    let client = OrbitContractClient::new(&env, &contract_id);
    client.create_vault_with_grace_period(&user, &merchant, &token, &10, &100, &50);
    env.ledger().set_timestamp(150);
    client.record_failed_pull(&merchant, &user);
    assert_eq!(client.get_vault(&user, &merchant).unwrap().payment_status, PAYMENT_DEFAULTED);
    assert!(client.try_pull_funds(&user, &merchant).is_err());
    client.merchant_cancel_vault(&merchant, &user);
    assert_eq!(client.get_vault(&user, &merchant), None);
}

#[test]
fn test_merchant_cancel_and_refund() {
    let env = Env::default();
    env.mock_all_auths();
    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let token = env.register_stellar_asset_contract_v2(token_admin).address();
    let token_client = TokenClient::new(&env, &token);
    StellarAssetClient::new(&env, &token).mint(&merchant, &100);
    let contract_id = env.register(OrbitContract, ());
    let client = OrbitContractClient::new(&env, &contract_id);
    client.create_vault(&user, &merchant, &token, &10, &100);
    client.merchant_cancel_vault(&merchant, &user);
    assert_eq!(client.get_vault(&user, &merchant), None);
    token_client.approve(&merchant, &client.address, &25, &2000);
    client.refund_subscriber(&merchant, &user, &token, &25);
    assert_eq!(token_client.balance(&user), 25);
}

#[test]
fn test_update_vault_terms_preserves_last_pull_and_uses_new_amount() {
    let env = Env::default();
    env.mock_all_auths();
    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let token = env.register_stellar_asset_contract_v2(token_admin).address();
    let token_client = TokenClient::new(&env, &token);
    StellarAssetClient::new(&env, &token).mint(&user, &100);
    let contract_id = env.register(OrbitContract, ());
    let client = OrbitContractClient::new(&env, &contract_id);
    token_client.approve(&user, &client.address, &100, &2000);
    client.create_vault(&user, &merchant, &token, &10, &100);
    env.ledger().set_timestamp(10);
    client.pull_funds(&user, &merchant);
    client.update_vault_terms(&user, &merchant, &25, &200);
    let vault = client.get_vault(&user, &merchant).unwrap();
    assert_eq!(vault.last_pull_timestamp, 10);
    assert_eq!(vault.amount_per_interval, 25);
    env.ledger().set_timestamp(210);
    client.pull_funds(&user, &merchant);
    assert_eq!(token_client.balance(&merchant), 35);
}

#[test]
fn test_stream_withdraws_linearly_and_at_completion() {
    let env = Env::default();
    env.mock_all_auths();
    let sender = Address::generate(&env);
    let recipient = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let token = env.register_stellar_asset_contract_v2(token_admin).address();
    let token_client = TokenClient::new(&env, &token);
    StellarAssetClient::new(&env, &token).mint(&sender, &100);
    let contract_id = env.register(OrbitContract, ());
    let client = OrbitContractClient::new(&env, &contract_id);
    token_client.approve(&sender, &client.address, &100, &2000);
    let stream_id = client.create_stream(&sender, &recipient, &token, &100, &100, &200);
    assert_eq!(token_client.balance(&sender), 0);
    env.ledger().set_timestamp(150);
    client.withdraw_from_stream(&stream_id, &25);
    assert_eq!(token_client.balance(&recipient), 25);
    env.ledger().set_timestamp(200);
    client.withdraw_from_stream(&stream_id, &75);
    assert_eq!(token_client.balance(&recipient), 100);
}

#[test]
fn test_stream_early_cancel_splits_vested_and_unvested_amounts() {
    let env = Env::default();
    env.mock_all_auths();
    let sender = Address::generate(&env);
    let recipient = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let token = env.register_stellar_asset_contract_v2(token_admin).address();
    let token_client = TokenClient::new(&env, &token);
    StellarAssetClient::new(&env, &token).mint(&sender, &100);
    let contract_id = env.register(OrbitContract, ());
    let client = OrbitContractClient::new(&env, &contract_id);
    token_client.approve(&sender, &client.address, &100, &2000);
    let stream_id = client.create_stream(&sender, &recipient, &token, &100, &100, &200);
    env.ledger().set_timestamp(120);
    client.cancel_stream(&stream_id);
    assert_eq!(token_client.balance(&recipient), 20);
    assert_eq!(token_client.balance(&sender), 80);
}

// ----------------------------------------------------------------------------
// ISSUE #93: SUBSCRIPTION PAUSE & RESUME
// ----------------------------------------------------------------------------

#[test]
fn test_pause_blocks_pull_and_resume_restores_billing() {
    let env = Env::default();
    env.mock_all_auths();

    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token_admin = Address::generate(&env);

    let token_address = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = TokenClient::new(&env, &token_address);
    let token_admin_client = StellarAssetClient::new(&env, &token_address);
    token_admin_client.mint(&user, &100_0000000);

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    let amount = 29_0000000;
    let interval = 86400;

    token_client.approve(&user, &orbit_client.address, &100_0000000, &2000000);
    orbit_client.create_vault(&user, &merchant, &token_address, &amount, &interval);

    // First settlement at t = 1000
    env.ledger().set_timestamp(1000);
    orbit_client.pull_funds(&user, &merchant);
    assert_eq!(token_client.balance(&merchant), amount);

    // The subscriber freezes the vault
    orbit_client.pause_vault(&user, &merchant);
    let paused = orbit_client
        .get_vault(&user, &merchant)
        .expect("vault should exist");
    assert!(paused.is_paused);

    // A full interval later the merchant still cannot pull while paused
    env.ledger().set_timestamp(1000 + interval);
    let res = orbit_client.try_pull_funds(&user, &merchant);
    assert!(res.is_err());
    assert_eq!(token_client.balance(&merchant), amount);
    assert_eq!(token_client.balance(&user), 100_0000000 - amount);

    // Resume restarts the billing clock at "now" so the interval that elapsed
    // during the pause is not back-billed
    orbit_client.resume_vault(&user, &merchant);
    let resumed = orbit_client
        .get_vault(&user, &merchant)
        .expect("vault should exist");
    assert!(!resumed.is_paused);
    assert_eq!(resumed.last_pull_timestamp, 1000 + interval);

    // Pulling immediately after the resume is rejected (no retro-active billing)
    let res = orbit_client.try_pull_funds(&user, &merchant);
    assert!(res.is_err());

    // A full interval after the resume the merchant can bill again
    env.ledger().set_timestamp(1000 + 2 * interval);
    orbit_client.pull_funds(&user, &merchant);
    assert_eq!(token_client.balance(&merchant), amount * 2);
    assert_eq!(token_client.balance(&user), 100_0000000 - amount * 2);
}

#[test]
fn test_pause_vault_authorized_by_merchant_fails() {
    let env = Env::default();
    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token = Address::generate(&env);

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    env.mock_all_auths();
    orbit_client.create_vault(&user, &merchant, &token, &29_0000000, &86400);

    // Only the merchant is authorized, but pause_vault requires the user
    let res = orbit_client
        .mock_auths(&[MockAuth {
            address: &merchant,
            invoke: &MockAuthInvoke {
                contract: &orbit_client.address,
                fn_name: "pause_vault",
                args: (&user, &merchant).into_val(&env),
                sub_invokes: &[],
            },
        }])
        .try_pause_vault(&user, &merchant);
    assert!(res.is_err());

    let vault = orbit_client
        .get_vault(&user, &merchant)
        .expect("vault should exist");
    assert!(!vault.is_paused);
}

#[test]
fn test_resume_vault_authorized_by_merchant_fails() {
    let env = Env::default();
    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token = Address::generate(&env);

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    env.mock_all_auths();
    orbit_client.create_vault(&user, &merchant, &token, &29_0000000, &86400);
    orbit_client.pause_vault(&user, &merchant);

    let res = orbit_client
        .mock_auths(&[MockAuth {
            address: &merchant,
            invoke: &MockAuthInvoke {
                contract: &orbit_client.address,
                fn_name: "resume_vault",
                args: (&user, &merchant).into_val(&env),
                sub_invokes: &[],
            },
        }])
        .try_resume_vault(&user, &merchant);
    assert!(res.is_err());

    let vault = orbit_client
        .get_vault(&user, &merchant)
        .expect("vault should exist");
    assert!(vault.is_paused);
}

#[test]
fn test_cancel_vault_works_while_paused() {
    let env = Env::default();
    env.mock_all_auths();

    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token_admin = Address::generate(&env);

    let token_address = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = TokenClient::new(&env, &token_address);
    let token_admin_client = StellarAssetClient::new(&env, &token_address);
    token_admin_client.mint(&user, &100_0000000);

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    token_client.approve(&user, &orbit_client.address, &100_0000000, &2000000);
    orbit_client.create_vault(&user, &merchant, &token_address, &29_0000000, &86400);

    orbit_client.pause_vault(&user, &merchant);
    orbit_client.cancel_vault(&user, &merchant);

    assert_eq!(orbit_client.get_vault(&user, &merchant), None);
    let res = orbit_client.try_pull_funds(&user, &merchant);
    assert!(res.is_err());
    assert_eq!(token_client.balance(&merchant), 0);
    assert_eq!(token_client.balance(&user), 100_0000000);
}

#[test]
fn test_pause_and_resume_of_unknown_vault_fails() {
    let env = Env::default();
    env.mock_all_auths();

    let user = Address::generate(&env);
    let merchant = Address::generate(&env);

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    assert!(orbit_client.try_pause_vault(&user, &merchant).is_err());
    assert!(orbit_client.try_resume_vault(&user, &merchant).is_err());
}

#[test]
fn test_pause_and_resume_events_published() {
    let env = Env::default();
    env.mock_all_auths();

    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token = Address::generate(&env);

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    let amount = 29_0000000;
    let interval = 86400;

    orbit_client.create_vault(&user, &merchant, &token, &amount, &interval);

    orbit_client.pause_vault(&user, &merchant);
    let paused_topics = soroban_sdk::vec![
        &env,
        Symbol::new(&env, "vault_paused").to_val(),
        user.clone().to_val(),
        merchant.clone().to_val(),
    ];
    let expected_pause = soroban_sdk::vec![
        &env,
        (
            orbit_client.address.clone(),
            paused_topics,
            ().into_val(&env),
        ),
    ];
    assert_eq!(env.events().all(), expected_pause);

    orbit_client.resume_vault(&user, &merchant);
    let resumed_topics = soroban_sdk::vec![
        &env,
        Symbol::new(&env, "vault_resumed").to_val(),
        user.clone().to_val(),
        merchant.clone().to_val(),
    ];
    let expected_resume = soroban_sdk::vec![
        &env,
        (
            orbit_client.address.clone(),
            resumed_topics,
            ().into_val(&env),
        ),
    ];
    assert_eq!(env.events().all(), expected_resume);
}

// ----------------------------------------------------------------------------
// ISSUE #94: MONOTONIC PULL EPOCH COUNTER
// ----------------------------------------------------------------------------

#[test]
fn test_pull_sequence_tracks_billing_cycles_monotonically() {
    let env = Env::default();
    env.mock_all_auths();

    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token_admin = Address::generate(&env);

    let token_address = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = TokenClient::new(&env, &token_address);
    let token_admin_client = StellarAssetClient::new(&env, &token_address);
    token_admin_client.mint(&user, &100_0000000);

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    let amount = 29_0000000;
    let interval = 86400;

    token_client.approve(&user, &orbit_client.address, &100_0000000, &2000000);
    orbit_client.create_vault(&user, &merchant, &token_address, &amount, &interval);

    env.ledger().set_timestamp(1000);
    orbit_client.pull_funds(&user, &merchant);
    assert_eq!(
        orbit_client
            .get_vault(&user, &merchant)
            .expect("vault should exist")
            .pull_sequence,
        0
    );

    env.ledger().set_timestamp(1000 + interval);
    orbit_client.pull_funds(&user, &merchant);
    assert_eq!(
        orbit_client
            .get_vault(&user, &merchant)
            .expect("vault should exist")
            .pull_sequence,
        1
    );

    env.ledger().set_timestamp(1000 + 2 * interval);
    orbit_client.pull_funds(&user, &merchant);
    assert_eq!(
        orbit_client
            .get_vault(&user, &merchant)
            .expect("vault should exist")
            .pull_sequence,
        2
    );

    assert_eq!(token_client.balance(&merchant), amount * 3);
    assert_eq!(token_client.balance(&user), 100_0000000 - amount * 3);
}

#[test]
fn test_same_epoch_second_pull_rejected_even_when_timestamp_check_is_bypassed() {
    let env = Env::default();
    env.mock_all_auths();

    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token_admin = Address::generate(&env);

    let token_address = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = TokenClient::new(&env, &token_address);
    let token_admin_client = StellarAssetClient::new(&env, &token_address);
    token_admin_client.mint(&user, &100_0000000);

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    let amount = 29_0000000;
    let interval = 86400;

    token_client.approve(&user, &orbit_client.address, &100_0000000, &2000000);
    orbit_client.create_vault(&user, &merchant, &token_address, &amount, &interval);

    // Settle at ledger timestamp 0: last_pull_timestamp stays 0, which
    // bypasses the legacy "Too early to pull funds" check entirely.
    orbit_client.pull_funds(&user, &merchant);
    assert_eq!(token_client.balance(&merchant), amount);

    // Later in the SAME computed cycle the epoch guard alone must reject the
    // second pull, keeping the single-pull-per-cycle invariant deterministic.
    env.ledger().set_timestamp(interval / 2);
    let res = orbit_client.try_pull_funds(&user, &merchant);
    assert!(res.is_err());
    assert_eq!(token_client.balance(&merchant), amount);
    assert_eq!(token_client.balance(&user), 100_0000000 - amount);

    // The next cycle opens and billing resumes exactly once
    env.ledger().set_timestamp(interval);
    orbit_client.pull_funds(&user, &merchant);
    assert_eq!(token_client.balance(&merchant), amount * 2);
    assert_eq!(
        orbit_client
            .get_vault(&user, &merchant)
            .expect("vault should exist")
            .pull_sequence,
        1
    );
}

#[test]
fn test_clock_jitter_cannot_double_pull_within_an_epoch() {
    let env = Env::default();
    env.mock_all_auths();

    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token_admin = Address::generate(&env);

    let token_address = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = TokenClient::new(&env, &token_address);
    let token_admin_client = StellarAssetClient::new(&env, &token_address);
    token_admin_client.mint(&user, &100_0000000);

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    let amount = 29_0000000;
    let interval = 86400;

    token_client.approve(&user, &orbit_client.address, &100_0000000, &2000000);
    orbit_client.create_vault(&user, &merchant, &token_address, &amount, &interval);

    // Settle inside billing cycle 1
    env.ledger().set_timestamp(100_000);
    orbit_client.pull_funds(&user, &merchant);
    assert_eq!(
        orbit_client
            .get_vault(&user, &merchant)
            .expect("vault should exist")
            .pull_sequence,
        1
    );

    // Sequencer clock steps backwards: no pull can be smuggled through
    env.ledger().set_timestamp(50_000);
    assert!(orbit_client.try_pull_funds(&user, &merchant).is_err());

    // Still inside the same computed cycle as the last settlement
    env.ledger().set_timestamp(172_799);
    assert!(orbit_client.try_pull_funds(&user, &merchant).is_err());

    assert_eq!(token_client.balance(&merchant), amount);

    // The next cycle opens exactly one interval after the settlement
    env.ledger().set_timestamp(100_000 + interval);
    orbit_client.pull_funds(&user, &merchant);
    assert_eq!(
        orbit_client
            .get_vault(&user, &merchant)
            .expect("vault should exist")
            .pull_sequence,
        2
    );

    // A duplicate attempt at the very same instant is rejected deterministically
    env.ledger().set_timestamp(100_000 + interval);
    assert!(orbit_client.try_pull_funds(&user, &merchant).is_err());

    assert_eq!(token_client.balance(&merchant), amount * 2);
    assert_eq!(token_client.balance(&user), 100_0000000 - amount * 2);
}

// ----------------------------------------------------------------------------
// ISSUE #95: DECENTRALIZED PLAN CATALOG & PLAN-LINKED VAULTS
// ----------------------------------------------------------------------------

#[test]
fn test_register_plan_and_read_it_back() {
    let env = Env::default();
    env.mock_all_auths();

    let merchant = Address::generate(&env);
    let token = Address::generate(&env);
    let plan_id = String::from_str(&env, "plan-pro");
    let name = String::from_str(&env, "Pro Plan");

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    // Unknown plan ids read back as None without panicking
    assert_eq!(
        orbit_client.get_plan(&String::from_str(&env, "missing")),
        None
    );

    orbit_client.register_plan(&merchant, &plan_id, &token, &29_0000000, &86400, &name);

    let plan = orbit_client.get_plan(&plan_id).expect("plan should exist");
    assert_eq!(plan.merchant, merchant);
    assert_eq!(plan.token, token);
    assert_eq!(plan.amount_per_interval, 29_0000000);
    assert_eq!(plan.interval_seconds, 86400);
    assert_eq!(plan.name, name);
    assert!(plan.is_active);
}

#[test]
fn test_register_plan_requires_merchant_authorization() {
    let env = Env::default();

    let merchant = Address::generate(&env);
    let other = Address::generate(&env);
    let token = Address::generate(&env);
    let plan_id = String::from_str(&env, "plan-pro");
    let name = String::from_str(&env, "Pro Plan");

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    let res = orbit_client
        .mock_auths(&[MockAuth {
            address: &other,
            invoke: &MockAuthInvoke {
                contract: &orbit_client.address,
                fn_name: "register_plan",
                args: (&merchant, &plan_id, &token, &29_0000000, &86400, &name).into_val(&env),
                sub_invokes: &[],
            },
        }])
        .try_register_plan(&merchant, &plan_id, &token, &29_0000000, &86400, &name);
    assert!(res.is_err());
    assert_eq!(orbit_client.get_plan(&plan_id), None);
}

#[test]
#[should_panic]
fn test_register_plan_rejects_zero_amount() {
    let env = Env::default();
    env.mock_all_auths();

    let merchant = Address::generate(&env);
    let token = Address::generate(&env);

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    orbit_client.register_plan(
        &merchant,
        &String::from_str(&env, "plan-pro"),
        &token,
        &0,
        &86400,
        &String::from_str(&env, "Pro Plan"),
    );
}

#[test]
#[should_panic]
fn test_register_plan_rejects_zero_interval() {
    let env = Env::default();
    env.mock_all_auths();

    let merchant = Address::generate(&env);
    let token = Address::generate(&env);

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    orbit_client.register_plan(
        &merchant,
        &String::from_str(&env, "plan-pro"),
        &token,
        &29_0000000,
        &0,
        &String::from_str(&env, "Pro Plan"),
    );
}

#[test]
fn test_register_plan_rejects_plan_id_owned_by_another_merchant() {
    let env = Env::default();
    env.mock_all_auths();

    let merchant_a = Address::generate(&env);
    let merchant_b = Address::generate(&env);
    let token = Address::generate(&env);
    let plan_id = String::from_str(&env, "plan-pro");
    let name = String::from_str(&env, "Pro Plan");

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    orbit_client.register_plan(&merchant_a, &plan_id, &token, &29_0000000, &86400, &name);

    // Another merchant cannot hijack the id
    let res = orbit_client.try_register_plan(
        &merchant_b,
        &plan_id,
        &token,
        &40_0000000,
        &172800,
        &String::from_str(&env, "Hijacked"),
    );
    assert!(res.is_err());

    let plan = orbit_client.get_plan(&plan_id).expect("plan should exist");
    assert_eq!(plan.merchant, merchant_a);
    assert_eq!(plan.amount_per_interval, 29_0000000);

    // The owner can refresh the terms, which also reactivates the plan
    orbit_client.deactivate_plan(&merchant_a, &plan_id);
    assert!(
        !orbit_client
            .get_plan(&plan_id)
            .expect("plan exists")
            .is_active
    );

    orbit_client.register_plan(&merchant_a, &plan_id, &token, &40_0000000, &172800, &name);
    let plan = orbit_client.get_plan(&plan_id).expect("plan should exist");
    assert_eq!(plan.amount_per_interval, 40_0000000);
    assert_eq!(plan.interval_seconds, 172800);
    assert!(plan.is_active);
}

#[test]
fn test_deactivate_plan_requires_owning_merchant() {
    let env = Env::default();
    env.mock_all_auths();

    let merchant = Address::generate(&env);
    let other = Address::generate(&env);
    let token = Address::generate(&env);
    let plan_id = String::from_str(&env, "plan-pro");
    let name = String::from_str(&env, "Pro Plan");

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    orbit_client.register_plan(&merchant, &plan_id, &token, &29_0000000, &86400, &name);

    let res = orbit_client
        .mock_auths(&[MockAuth {
            address: &other,
            invoke: &MockAuthInvoke {
                contract: &orbit_client.address,
                fn_name: "deactivate_plan",
                args: (&other, &plan_id).into_val(&env),
                sub_invokes: &[],
            },
        }])
        .try_deactivate_plan(&other, &plan_id);
    assert!(res.is_err());
    assert!(
        orbit_client
            .get_plan(&plan_id)
            .expect("plan exists")
            .is_active
    );
}

#[test]
fn test_deactivate_plan_blocks_new_vaults_but_existing_vaults_keep_billing() {
    let env = Env::default();
    env.mock_all_auths();

    let user = Address::generate(&env);
    let new_user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token_admin = Address::generate(&env);

    let token_address = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = TokenClient::new(&env, &token_address);
    let token_admin_client = StellarAssetClient::new(&env, &token_address);
    token_admin_client.mint(&user, &100_0000000);

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    let amount = 29_0000000;
    let interval = 86400;
    let plan_id = String::from_str(&env, "plan-pro");

    orbit_client.register_plan(
        &merchant,
        &plan_id,
        &token_address,
        &amount,
        &interval,
        &String::from_str(&env, "Pro Plan"),
    );

    // A user opens a vault from the on-chain plan terms
    token_client.approve(&user, &orbit_client.address, &100_0000000, &2000000);
    orbit_client.create_vault_from_plan(&user, &plan_id);

    let vault = orbit_client
        .get_vault(&user, &merchant)
        .expect("vault should exist");
    assert_eq!(vault.token, token_address);
    assert_eq!(vault.amount_per_interval, amount);
    assert_eq!(vault.interval_seconds, interval);
    assert!(!vault.is_paused);

    env.ledger().set_timestamp(1000);
    orbit_client.pull_funds(&user, &merchant);
    assert_eq!(token_client.balance(&merchant), amount);

    // Merchant deactivates the plan: no NEW subscriptions, existing ones live
    orbit_client.deactivate_plan(&merchant, &plan_id);
    assert!(
        !orbit_client
            .get_plan(&plan_id)
            .expect("plan exists")
            .is_active
    );

    let res = orbit_client.try_create_vault_from_plan(&new_user, &plan_id);
    assert!(res.is_err());
    assert_eq!(orbit_client.get_vault(&new_user, &merchant), None);

    // The pre-existing vault keeps billing on schedule
    env.ledger().set_timestamp(1000 + interval);
    orbit_client.pull_funds(&user, &merchant);
    assert_eq!(token_client.balance(&merchant), amount * 2);
    assert_eq!(token_client.balance(&user), 100_0000000 - amount * 2);
}

#[test]
fn test_create_vault_from_plan_requires_user_authorization() {
    let env = Env::default();
    env.mock_all_auths();

    let user = Address::generate(&env);
    let other = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token = Address::generate(&env);
    let plan_id = String::from_str(&env, "plan-pro");

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    orbit_client.register_plan(
        &merchant,
        &plan_id,
        &token,
        &29_0000000,
        &86400,
        &String::from_str(&env, "Pro Plan"),
    );

    let res = orbit_client
        .mock_auths(&[MockAuth {
            address: &other,
            invoke: &MockAuthInvoke {
                contract: &orbit_client.address,
                fn_name: "create_vault_from_plan",
                args: (&user, &plan_id).into_val(&env),
                sub_invokes: &[],
            },
        }])
        .try_create_vault_from_plan(&user, &plan_id);
    assert!(res.is_err());
    assert_eq!(orbit_client.get_vault(&user, &merchant), None);
}

#[test]
fn test_create_vault_from_unknown_plan_fails() {
    let env = Env::default();
    env.mock_all_auths();

    let user = Address::generate(&env);
    let merchant = Address::generate(&env);

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    let res = orbit_client.try_create_vault_from_plan(&user, &String::from_str(&env, "missing"));
    assert!(res.is_err());
    assert_eq!(orbit_client.get_vault(&user, &merchant), None);
}

#[test]
fn test_deactivate_unknown_plan_fails() {
    let env = Env::default();
    env.mock_all_auths();

    let merchant = Address::generate(&env);

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    let res = orbit_client.try_deactivate_plan(&merchant, &String::from_str(&env, "missing"));
    assert!(res.is_err());
}

#[test]
fn test_create_vault_from_plan_emits_vault_created_event() {
    let env = Env::default();
    env.mock_all_auths();

    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token = Address::generate(&env);
    let plan_id = String::from_str(&env, "plan-pro");
    let name = String::from_str(&env, "Pro Plan");

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    let amount = 29_0000000;
    let interval = 86400;

    orbit_client.register_plan(&merchant, &plan_id, &token, &amount, &interval, &name);
    orbit_client.create_vault_from_plan(&user, &plan_id);

    let vault_topics = soroban_sdk::vec![
        &env,
        Symbol::new(&env, "vault_created").to_val(),
        user.clone().to_val(),
        merchant.clone().to_val(),
    ];
    let expected = soroban_sdk::vec![
        &env,
        (
            orbit_client.address.clone(),
            vault_topics,
            (token.clone(), amount, interval, 0u64).into_val(&env),
        ),
    ];
    assert_eq!(env.events().all(), expected);
}

// ----------------------------------------------------------------------------
// ISSUE #46: METERED (USAGE-BASED) BILLING
// ----------------------------------------------------------------------------

const METERED_CAP: i128 = 20_0000000; // 20 USDC per interval
const METERED_INTERVAL: u64 = 7 * 24 * 60 * 60; // weekly

struct MeteredWorld<'a> {
    env: Env,
    user: Address,
    merchant: Address,
    token_address: Address,
    token_client: TokenClient<'a>,
    orbit_client: OrbitContractClient<'a>,
}

fn metered_world<'a>() -> MeteredWorld<'a> {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let token_address = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = TokenClient::new(&env, &token_address);
    StellarAssetClient::new(&env, &token_address).mint(&user, &100_0000000);

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);
    token_client.approve(&user, &orbit_client.address, &100_0000000, &2000000);
    orbit_client.create_metered_vault(
        &user,
        &merchant,
        &token_address,
        &METERED_CAP,
        &METERED_INTERVAL,
    );

    MeteredWorld {
        env,
        user,
        merchant,
        token_address,
        token_client,
        orbit_client,
    }
}

#[test]
fn test_create_metered_vault_stores_terms() {
    let w = metered_world();
    let vault = w
        .orbit_client
        .get_metered_vault(&w.user, &w.merchant)
        .unwrap();
    assert_eq!(vault.token, w.token_address);
    assert_eq!(vault.max_amount_per_interval, METERED_CAP);
    assert_eq!(vault.interval_seconds, METERED_INTERVAL);
    assert_eq!(vault.current_epoch_start, 1_000);
    assert_eq!(vault.epoch_spent, 0);
    // A metered vault doesn't create a fixed-interval vault.
    assert!(w.orbit_client.get_vault(&w.user, &w.merchant).is_none());
}

#[test]
fn test_metered_multiple_partial_pulls_within_interval() {
    let w = metered_world();
    w.orbit_client
        .pull_metered_funds(&w.user, &w.merchant, &3_4500000); // $3.45
    w.env.ledger().set_timestamp(2_000);
    w.orbit_client
        .pull_metered_funds(&w.user, &w.merchant, &8_2000000); // $8.20
    w.orbit_client
        .pull_metered_funds(&w.user, &w.merchant, &8_3500000); // exactly up to the cap

    assert_eq!(w.token_client.balance(&w.merchant), 20_0000000);
    assert_eq!(w.token_client.balance(&w.user), 80_0000000);
    let vault = w
        .orbit_client
        .get_metered_vault(&w.user, &w.merchant)
        .unwrap();
    assert_eq!(vault.epoch_spent, METERED_CAP);
    assert_eq!(vault.current_epoch_start, 1_000, "still the same epoch");
}

#[test]
#[should_panic(expected = "Interval spending limit exceeded")]
fn test_metered_pull_exceeding_remaining_allowance_panics() {
    let w = metered_world();
    w.orbit_client
        .pull_metered_funds(&w.user, &w.merchant, &15_0000000);
    w.orbit_client
        .pull_metered_funds(&w.user, &w.merchant, &5_0000001); // 1 stroop over
}

#[test]
#[should_panic(expected = "Interval spending limit exceeded")]
fn test_metered_allowance_does_not_reset_before_interval_elapses() {
    let w = metered_world();
    w.orbit_client
        .pull_metered_funds(&w.user, &w.merchant, &METERED_CAP);
    w.env.ledger().set_timestamp(1_000 + METERED_INTERVAL - 1);
    w.orbit_client.pull_metered_funds(&w.user, &w.merchant, &1);
}

#[test]
fn test_metered_epoch_rolls_over_and_resets_allowance() {
    let w = metered_world();
    w.orbit_client
        .pull_metered_funds(&w.user, &w.merchant, &METERED_CAP);

    let next_epoch = 1_000 + METERED_INTERVAL;
    w.env.ledger().set_timestamp(next_epoch);
    w.orbit_client
        .pull_metered_funds(&w.user, &w.merchant, &4_0000000);

    let vault = w
        .orbit_client
        .get_metered_vault(&w.user, &w.merchant)
        .unwrap();
    assert_eq!(vault.current_epoch_start, next_epoch);
    assert_eq!(vault.epoch_spent, 4_0000000);
    assert_eq!(w.token_client.balance(&w.merchant), 24_0000000);
}

#[test]
fn test_cancel_vault_removes_metered_vault() {
    let w = metered_world();
    w.orbit_client.cancel_vault(&w.user, &w.merchant);
    assert!(w
        .orbit_client
        .get_metered_vault(&w.user, &w.merchant)
        .is_none());
}

#[test]
#[should_panic(expected = "Metered vault does not exist")]
fn test_pull_after_cancelling_metered_vault_fails() {
    let w = metered_world();
    w.orbit_client.cancel_vault(&w.user, &w.merchant);
    w.orbit_client
        .pull_metered_funds(&w.user, &w.merchant, &1_0000000);
}

#[test]
fn test_metered_and_fixed_vaults_for_same_pair_are_independent() {
    let w = metered_world();
    w.orbit_client
        .create_vault(&w.user, &w.merchant, &w.token_address, &29_0000000, &86400);
    w.orbit_client
        .pull_metered_funds(&w.user, &w.merchant, &5_0000000);
    assert_eq!(
        w.orbit_client
            .get_vault(&w.user, &w.merchant)
            .unwrap()
            .amount_per_interval,
        29_0000000
    );
    assert_eq!(
        w.orbit_client
            .get_metered_vault(&w.user, &w.merchant)
            .unwrap()
            .epoch_spent,
        5_0000000
    );

    // cancel_vault clears both.
    w.orbit_client.cancel_vault(&w.user, &w.merchant);
    assert!(w.orbit_client.get_vault(&w.user, &w.merchant).is_none());
    assert!(w
        .orbit_client
        .get_metered_vault(&w.user, &w.merchant)
        .is_none());
}

#[test]
#[should_panic(expected = "max_amount_per_interval must be positive")]
fn test_create_metered_vault_rejects_zero_cap() {
    let w = metered_world();
    w.orbit_client.create_metered_vault(
        &w.user,
        &w.merchant,
        &w.token_address,
        &0,
        &METERED_INTERVAL,
    );
}

#[test]
#[should_panic(expected = "interval_seconds must be greater than zero")]
fn test_create_metered_vault_rejects_zero_interval() {
    let w = metered_world();
    w.orbit_client
        .create_metered_vault(&w.user, &w.merchant, &w.token_address, &METERED_CAP, &0);
}

#[test]
#[should_panic(expected = "amount must be positive")]
fn test_metered_pull_rejects_non_positive_amount() {
    let w = metered_world();
    w.orbit_client.pull_metered_funds(&w.user, &w.merchant, &0);
}

#[test]
#[should_panic]
fn test_metered_pull_authorized_by_different_merchant_fails() {
    let w = metered_world();
    let other_merchant = Address::generate(&w.env);
    // `mock_auths` returns a client with per-call auth, so chain the call on it.
    w.orbit_client
        .mock_auths(&[MockAuth {
            address: &other_merchant,
            invoke: &MockAuthInvoke {
                contract: &w.orbit_client.address,
                fn_name: "pull_metered_funds",
                args: (&w.user, &w.merchant, 1_0000000i128).into_val(&w.env),
                sub_invokes: &[],
            },
        }])
        .pull_metered_funds(&w.user, &w.merchant, &1_0000000);
}

#[test]
#[should_panic]
fn test_create_metered_vault_requires_user_auth() {
    let env = Env::default();
    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token = Address::generate(&env);
    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);
    // No auths mocked: user.require_auth() must fail.
    orbit_client.create_metered_vault(&user, &merchant, &token, &METERED_CAP, &METERED_INTERVAL);
}

#[test]
fn test_metered_events_published_for_creation_and_pulls() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);
    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let token_address = env
        .register_stellar_asset_contract_v2(token_admin)
        .address();
    StellarAssetClient::new(&env, &token_address).mint(&user, &100_0000000);
    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);
    TokenClient::new(&env, &token_address).approve(
        &user,
        &orbit_client.address,
        &100_0000000,
        &2000000,
    );

    orbit_client.create_metered_vault(
        &user,
        &merchant,
        &token_address,
        &METERED_CAP,
        &METERED_INTERVAL,
    );
    let create_topics = soroban_sdk::vec![
        &env,
        Symbol::new(&env, "metered_vault_created").to_val(),
        user.clone().to_val(),
        merchant.clone().to_val(),
    ];
    let create_data = (token_address.clone(), METERED_CAP, METERED_INTERVAL).into_val(&env);
    assert_eq!(
        env.events().all().filter_by_contract(&orbit_client.address),
        soroban_sdk::vec![
            &env,
            (orbit_client.address.clone(), create_topics, create_data)
        ]
    );

    orbit_client.pull_metered_funds(&user, &merchant, &3_4500000);
    let pull_topics = soroban_sdk::vec![
        &env,
        Symbol::new(&env, "metered_funds_pulled").to_val(),
        user.clone().to_val(),
        merchant.clone().to_val(),
    ];
    let pull_data = (3_4500000i128, 3_4500000i128, METERED_CAP).into_val(&env);
    assert_eq!(
        env.events().all().filter_by_contract(&orbit_client.address),
        soroban_sdk::vec![&env, (orbit_client.address.clone(), pull_topics, pull_data)]
    );
}

// ----------------------------------------------------------------------------
// #123–126: Soulbound credentials, bitmap registry, swap and escrow settlement
// ----------------------------------------------------------------------------

#[test]
fn test_badge_issuance_renewal_expiration_and_revocation() {
    let env = Env::default();
    env.mock_all_auths();
    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let admin = Address::generate(&env);
    let token_id = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token = TokenClient::new(&env, &token_id);
    StellarAssetClient::new(&env, &token_id).mint(&user, &1_000);
    let id = env.register(OrbitContract, ());
    let client = OrbitContractClient::new(&env, &id);
    token.approve(&user, &id, &1_000, &2_000_000);

    client.create_vault(&user, &merchant, &token_id, &100, &100);
    let original = client.get_subscriber_badge(&user, &merchant).unwrap();
    assert!(original.active);
    assert_eq!(original.tier, 1);
    assert_eq!(original.subscriber_id, 0);
    assert_eq!(original.expiration_timestamp, 100);
    assert!(client.is_subscriber_active(&user, &merchant));
    assert_eq!(
        client.get_subscriber_bitmap_chunk(&merchant, &0).to_array()[0],
        1
    );

    client.set_subscriber_tier(&user, &merchant, &3);
    env.ledger().set_timestamp(20);
    client.pull_funds(&user, &merchant);
    let renewed = client.get_subscriber_badge(&user, &merchant).unwrap();
    assert_eq!(renewed.tier, 3);
    assert_eq!(renewed.expiration_timestamp, 120);
    env.ledger().set_timestamp(121);
    assert!(!client.is_subscriber_active(&user, &merchant));
    assert!(client.mark_subscription_default(&user, &merchant));
    assert!(
        !client
            .get_subscriber_badge(&user, &merchant)
            .unwrap()
            .active
    );
    assert_eq!(
        client.get_subscriber_bitmap_chunk(&merchant, &0).to_array()[0],
        0
    );

    client.cancel_vault(&user, &merchant);
    client.create_vault(&user, &merchant, &token_id, &100, &100);
    assert_eq!(
        client
            .get_subscriber_badge(&user, &merchant)
            .unwrap()
            .subscriber_id,
        0
    );
    assert_eq!(
        client.get_subscriber_bitmap_chunk(&merchant, &0).to_array()[0],
        1
    );
    client.cancel_vault(&user, &merchant);
    assert!(!client.is_subscriber_active(&user, &merchant));
    assert_eq!(
        client.get_subscriber_bitmap_chunk(&merchant, &0).to_array()[0],
        0
    );
}

#[test]
fn test_bitmap_chunk_boundary_and_neighbor_integrity() {
    let env = Env::default();
    env.mock_all_auths();
    let merchant = Address::generate(&env);
    let token = Address::generate(&env);
    let id = env.register(OrbitContract, ());
    let client = OrbitContractClient::new(&env, &id);
    let mut users = Vec::<Address>::new(&env);
    for _ in 0..258 {
        let user = Address::generate(&env);
        client.create_vault(&user, &merchant, &token, &10, &86400);
        users.push_back(user);
    }
    let first = client.get_subscriber_bitmap_chunk(&merchant, &0).to_array();
    let second = client.get_subscriber_bitmap_chunk(&merchant, &1).to_array();
    assert_eq!(first, [255u8; 32]);
    assert_eq!(second[0], 3);
    assert_eq!(second[1], 0);

    client.cancel_vault(&users.get(255).unwrap(), &merchant);
    client.cancel_vault(&users.get(256).unwrap(), &merchant);
    let first = client.get_subscriber_bitmap_chunk(&merchant, &0).to_array();
    let second = client.get_subscriber_bitmap_chunk(&merchant, &1).to_array();
    assert_eq!(first[31], 127);
    assert_eq!(first[30], 255);
    assert_eq!(second[0], 2);
    assert!(client.is_subscriber_active(&users.get(257).unwrap(), &merchant));
}

#[test]
fn test_escrow_claim_dispute_and_partial_arbitration() {
    let env = Env::default();
    env.mock_all_auths();
    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let arbitrator = Address::generate(&env);
    let admin = Address::generate(&env);
    let token_id = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token = TokenClient::new(&env, &token_id);
    StellarAssetClient::new(&env, &token_id).mint(&user, &1_000);
    let id = env.register(OrbitContract, ());
    let client = OrbitContractClient::new(&env, &id);
    token.approve(&user, &id, &1_000, &2_000_000);
    client.create_vault(&user, &merchant, &token_id, &100, &100);
    client.configure_escrow(&user, &merchant, &arbitrator, &30);

    env.ledger().set_timestamp(10);
    client.pull_funds(&user, &merchant);
    assert_eq!(token.balance(&merchant), 0);
    assert_eq!(token.balance(&id), 100);
    assert_eq!(client.get_escrow(&0).unwrap().release_at, 40);
    assert!(client.try_claim_escrow_funds(&merchant, &0).is_err());
    client.raise_dispute(&user, &merchant, &0);
    env.ledger().set_timestamp(41);
    assert!(client.try_claim_escrow_funds(&merchant, &0).is_err());
    assert!(client.try_resolve_dispute(&user, &0, &50).is_err());
    assert!(client.try_resolve_dispute(&arbitrator, &0, &101).is_err());
    client.resolve_dispute(&arbitrator, &0, &25);
    assert_eq!(token.balance(&user), 925);
    assert_eq!(token.balance(&merchant), 75);
    assert!(client.get_escrow(&0).is_none());

    env.ledger().set_timestamp(110);
    client.pull_funds(&user, &merchant);
    assert_eq!(token.balance(&merchant), 75);
    assert!(client.try_claim_escrow_funds(&merchant, &1).is_err());
    env.ledger().set_timestamp(141);
    client.claim_escrow_funds(&merchant, &1);
    assert_eq!(token.balance(&merchant), 175);
    assert!(client.get_escrow(&1).is_none());
}

#[soroban_sdk::contract]
pub struct TestExactOutputRouter;

#[soroban_sdk::contractimpl]
impl TestExactOutputRouter {
    pub fn swap_exact_output(
        env: Env,
        payment_token: Address,
        settlement_token: Address,
        amount_out: i128,
        max_amount_in: i128,
        recipient: Address,
    ) -> i128 {
        let spent = amount_out.checked_mul(2).unwrap();
        assert!(
            spent <= max_amount_in,
            "Insufficient max input for router quote"
        );
        let router = env.current_contract_address();
        token::Client::new(&env, &settlement_token).transfer(&router, &recipient, &amount_out);
        if max_amount_in > spent {
            token::Client::new(&env, &payment_token).transfer(
                &router,
                &recipient,
                &(max_amount_in - spent),
            );
        }
        spent
    }
}

#[test]
fn test_cross_asset_swap_exact_output_refund_and_slippage_rejection() {
    let env = Env::default();
    env.mock_all_auths();
    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let admin = Address::generate(&env);
    let payment = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let settlement = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let payment_token = TokenClient::new(&env, &payment);
    let settlement_token = TokenClient::new(&env, &settlement);
    StellarAssetClient::new(&env, &payment).mint(&user, &1_000);
    let router = env.register(TestExactOutputRouter, ());
    StellarAssetClient::new(&env, &settlement).mint(&router, &1_000);
    let id = env.register(OrbitContract, ());
    let client = OrbitContractClient::new(&env, &id);
    payment_token.approve(&user, &id, &1_000, &2_000_000);
    client.create_vault(&user, &merchant, &settlement, &100, &100);
    client.configure_swap(&user, &merchant, &payment, &router, &250);
    env.ledger().set_timestamp(20);
    client.pull_funds(&user, &merchant);
    assert_eq!(payment_token.balance(&user), 800);
    assert_eq!(payment_token.balance(&router), 200);
    assert_eq!(settlement_token.balance(&merchant), 100);
    assert_eq!(settlement_token.balance(&id), 0);

    // Change the authorized quote cap to below the router's input requirement.
    client.configure_swap(&user, &merchant, &payment, &router, &150);
    env.ledger().set_timestamp(121);
    assert!(client.try_pull_funds(&user, &merchant).is_err());
    assert_eq!(payment_token.balance(&user), 800);
    assert_eq!(settlement_token.balance(&merchant), 100);
}

// ----------------------------------------------------------------------------
// #188: PAYROLL ESCROW EXPIRATION & UNCLAIMED RECLAMATION
// ----------------------------------------------------------------------------

/// Sets up a funded sender, a recipient and an orbit contract with a 100 token
/// allowance. Returns (sender, recipient, token, orbit contract id).
fn payroll_setup(env: &Env) -> (Address, Address, Address, Address) {
    env.mock_all_auths();
    env.ledger().with_mut(|l| l.timestamp = 1_000);

    let sender = Address::generate(env);
    let recipient = Address::generate(env);
    let token_admin = Address::generate(env);
    let token = env
        .register_stellar_asset_contract_v2(token_admin)
        .address();
    StellarAssetClient::new(env, &token).mint(&sender, &100_0000000);

    let contract_id = env.register(OrbitContract, ());
    TokenClient::new(env, &token).approve(&sender, &contract_id, &100_0000000, &2000000);
    (sender, recipient, token, contract_id)
}

#[test]
fn test_payroll_reclaim_returns_only_unclaimed_balance_after_expiration() {
    let env = Env::default();
    let (sender, recipient, token, contract_id) = payroll_setup(&env);
    let orbit = OrbitContractClient::new(&env, &contract_id);
    let token_client = TokenClient::new(&env, &token);

    let expiration = 2_000;
    let id = orbit.create_payroll_escrow(&sender, &recipient, &token, &60_0000000, &expiration);

    // Recipient claims 25 of the 60 before the window closes.
    orbit.claim_payroll_escrow(&recipient, &id, &25_0000000);
    assert_eq!(token_client.balance(&recipient), 25_0000000);

    // The sender cannot reclaim while the claim window is still open.
    assert!(orbit.try_reclaim_unclaimed_payroll(&sender, &id).is_err());

    env.ledger().with_mut(|l| l.timestamp = expiration + 1);
    let reclaimed = orbit.reclaim_unclaimed_payroll(&sender, &id);

    assert_eq!(reclaimed, 35_0000000);
    assert_eq!(token_client.balance(&sender), 100_0000000 - 25_0000000);
    let escrow = orbit.get_payroll_escrow(&id).unwrap();
    assert!(escrow.reclaimed);
    assert_eq!(escrow.claimed_amount, 25_0000000);
}

#[test]
fn test_payroll_claim_rejected_once_expiration_has_passed() {
    let env = Env::default();
    let (sender, recipient, token, contract_id) = payroll_setup(&env);
    let orbit = OrbitContractClient::new(&env, &contract_id);

    let id = orbit.create_payroll_escrow(&sender, &recipient, &token, &10_0000000, &2_000);

    env.ledger().with_mut(|l| l.timestamp = 2_001);
    assert!(orbit
        .try_claim_payroll_escrow(&recipient, &id, &1_0000000)
        .is_err());
}

#[test]
fn test_payroll_reclaim_is_one_shot_and_blocks_later_claims() {
    let env = Env::default();
    let (sender, recipient, token, contract_id) = payroll_setup(&env);
    let orbit = OrbitContractClient::new(&env, &contract_id);
    let token_client = TokenClient::new(&env, &token);

    let id = orbit.create_payroll_escrow(&sender, &recipient, &token, &10_0000000, &2_000);

    env.ledger().with_mut(|l| l.timestamp = 2_001);
    orbit.reclaim_unclaimed_payroll(&sender, &id);

    assert!(orbit.try_reclaim_unclaimed_payroll(&sender, &id).is_err());
    assert!(orbit
        .try_claim_payroll_escrow(&recipient, &id, &1_0000000)
        .is_err());
    assert_eq!(token_client.balance(&sender), 100_0000000);
}

#[test]
fn test_payroll_reclaim_requires_original_sender() {
    let env = Env::default();
    let (sender, recipient, token, contract_id) = payroll_setup(&env);
    let orbit = OrbitContractClient::new(&env, &contract_id);

    let id = orbit.create_payroll_escrow(&sender, &recipient, &token, &10_0000000, &2_000);
    env.ledger().with_mut(|l| l.timestamp = 2_001);

    let stranger = Address::generate(&env);
    assert!(orbit.try_reclaim_unclaimed_payroll(&stranger, &id).is_err());
}

#[test]
fn test_payroll_reclaim_fails_when_fully_claimed() {
    let env = Env::default();
    let (sender, recipient, token, contract_id) = payroll_setup(&env);
    let orbit = OrbitContractClient::new(&env, &contract_id);

    let id = orbit.create_payroll_escrow(&sender, &recipient, &token, &10_0000000, &2_000);
    orbit.claim_payroll_escrow(&recipient, &id, &10_0000000);

    env.ledger().with_mut(|l| l.timestamp = 2_001);
    assert!(orbit.try_reclaim_unclaimed_payroll(&sender, &id).is_err());
}

#[test]
fn test_payroll_create_rejects_past_expiration_and_zero_amount() {
    let env = Env::default();
    let (sender, recipient, token, contract_id) = payroll_setup(&env);
    let orbit = OrbitContractClient::new(&env, &contract_id);

    assert!(orbit
        .try_create_payroll_escrow(&sender, &recipient, &token, &1_0000000, &1_000)
        .is_err());
    assert!(orbit
        .try_create_payroll_escrow(&sender, &recipient, &token, &0, &2_000)
        .is_err());
}

// ----------------------------------------------------------------------------
// #191: SWAP-AND-SUBSCRIBE WITH MERCHANT-APPROVED ROUTERS
// ----------------------------------------------------------------------------

#[test]
fn test_create_vault_with_swap_creates_vault_and_payment_allowance_in_one_call() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|l| l.timestamp = 1_000);

    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let router = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let payment_token = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let settlement_token = env
        .register_stellar_asset_contract_v2(token_admin)
        .address();
    StellarAssetClient::new(&env, &payment_token).mint(&user, &50_0000000);

    let contract_id = env.register(OrbitContract, ());
    let orbit = OrbitContractClient::new(&env, &contract_id);
    orbit.approve_swap_router(&merchant, &router);

    orbit.create_vault_with_swap(
        &user,
        &merchant,
        &payment_token,
        &settlement_token,
        &router,
        &10_0000000,
        &(30 * 24 * 60 * 60),
        &12_0000000,
        &12_0000000,
        &2_000_000,
    );

    let vault = orbit.get_vault(&user, &merchant).unwrap();
    assert_eq!(vault.token, settlement_token);
    assert_eq!(vault.amount_per_interval, 10_0000000);
    assert_eq!(
        TokenClient::new(&env, &payment_token).allowance(&user, &contract_id),
        12_0000000
    );
}

#[test]
fn test_create_vault_with_swap_rejects_router_the_merchant_has_not_approved() {
    let env = Env::default();
    env.mock_all_auths();

    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let approved_router = Address::generate(&env);
    let rogue_router = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let payment_token = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let settlement_token = env
        .register_stellar_asset_contract_v2(token_admin)
        .address();

    let contract_id = env.register(OrbitContract, ());
    let orbit = OrbitContractClient::new(&env, &contract_id);
    orbit.approve_swap_router(&merchant, &approved_router);

    assert!(orbit
        .try_create_vault_with_swap(
            &user,
            &merchant,
            &payment_token,
            &settlement_token,
            &rogue_router,
            &10_0000000,
            &(30 * 24 * 60 * 60),
            &12_0000000,
            &12_0000000,
            &2_000_000,
        )
        .is_err());
    assert!(orbit.get_vault(&user, &merchant).is_none());
}

// ----------------------------------------------------------------------------
// #127: ACCUMULATIVE PER-SHARE REVENUE POOLS
// ----------------------------------------------------------------------------

#[test]
fn test_revenue_pool_multiple_deposits_claims_and_rounding_carry() {
    let env = Env::default();
    env.mock_all_auths();
    let owner = Address::generate(&env);
    let depositor = Address::generate(&env);
    let shareholder_a = Address::generate(&env);
    let shareholder_b = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = TokenClient::new(&env, &token);
    StellarAssetClient::new(&env, &token).mint(&depositor, &100);
    let contract_id = env.register(OrbitContract, ());
    let orbit = OrbitContractClient::new(&env, &contract_id);
    token_client.approve(&depositor, &contract_id, &100, &2_000_000);

    let shareholders = soroban_sdk::vec![
        &env,
        (shareholder_a.clone(), 1u32),
        (shareholder_b.clone(), 2u32),
    ];
    orbit.create_revenue_pool(&owner, &7, &token, &shareholders);

    // One indivisible unit cannot yet be claimed, but is never discarded.
    orbit.deposit_to_pool(&depositor, &7, &token, &1);
    assert!(orbit
        .try_claim_shareholder_dividends(&shareholder_a, &7)
        .is_err());
    assert!(orbit
        .try_claim_shareholder_dividends(&shareholder_b, &7)
        .is_err());

    // The next two units combine with the carried scaled remainder to make an
    // exact 1:2 distribution.
    orbit.deposit_to_pool(&depositor, &7, &token, &2);
    assert_eq!(orbit.claim_shareholder_dividends(&shareholder_a, &7), 1);
    assert_eq!(orbit.claim_shareholder_dividends(&shareholder_b, &7), 2);

    // Variable claim timing retains whole-token dust until a later deposit.
    orbit.deposit_to_pool(&depositor, &7, &token, &10);
    assert_eq!(orbit.claim_shareholder_dividends(&shareholder_b, &7), 6);
    assert_eq!(orbit.claim_shareholder_dividends(&shareholder_a, &7), 3);
    assert_eq!(
        orbit
            .get_revenue_pool(&7)
            .unwrap()
            .pool
            .undistributed_balance,
        1
    );

    orbit.deposit_to_pool(&depositor, &7, &token, &2);
    assert_eq!(orbit.claim_shareholder_dividends(&shareholder_a, &7), 1);
    assert_eq!(orbit.claim_shareholder_dividends(&shareholder_b, &7), 2);
    let state = orbit.get_revenue_pool(&7).unwrap();
    assert_eq!(state.pool.undistributed_balance, 0);
    assert_eq!(token_client.balance(&shareholder_a), 5);
    assert_eq!(token_client.balance(&shareholder_b), 10);
}

// ----------------------------------------------------------------------------
// MERKLE PAYROLL DISTRIBUTION (#156)
// ----------------------------------------------------------------------------

/// Builds every layer of a Merkle tree over `leaves` (sorted-pair hashing, an
/// odd node is promoted unchanged). Layer 0 is the leaves, the last is the root.
fn merkle_layers(env: &Env, leaves: &[BytesN<32>]) -> std::vec::Vec<std::vec::Vec<BytesN<32>>> {
    let mut layers = std::vec![leaves.to_vec()];
    while layers.last().unwrap().len() > 1 {
        let prev = layers.last().unwrap();
        let next = prev
            .chunks(2)
            .map(|pair| match pair {
                [a, b] => merkle_hash_pair(env, a, b),
                [a] => a.clone(),
                _ => unreachable!(),
            })
            .collect();
        layers.push(next);
    }
    layers
}

fn merkle_proof(
    env: &Env,
    layers: &[std::vec::Vec<BytesN<32>>],
    mut index: usize,
) -> Vec<BytesN<32>> {
    let mut proof = Vec::new(env);
    for layer in &layers[..layers.len() - 1] {
        let sibling = index ^ 1;
        if sibling < layer.len() {
            proof.push_back(layer[sibling].clone());
        }
        index /= 2;
    }
    proof
}

/// Five recipients with distinct allocations (odd count exercises promotion).
/// Returns (recipients, amounts, layers).
fn merkle_fixture(
    env: &Env,
) -> (
    std::vec::Vec<Address>,
    std::vec::Vec<i128>,
    std::vec::Vec<std::vec::Vec<BytesN<32>>>,
) {
    let amounts = std::vec![10_0000000, 20_0000000, 5_0000000, 15_0000000, 30_0000000];
    let recipients: std::vec::Vec<Address> =
        amounts.iter().map(|_| Address::generate(env)).collect();
    let leaves: std::vec::Vec<BytesN<32>> = recipients
        .iter()
        .zip(&amounts)
        .map(|(r, a)| merkle_leaf(env, r, *a))
        .collect();
    let layers = merkle_layers(env, &leaves);
    (recipients, amounts, layers)
}

/// Whether `contract` emitted an event whose first topic is the `name` symbol.
fn has_event(env: &Env, contract: &Address, name: &str) -> bool {
    let val: soroban_sdk::Val = Symbol::new(env, name).into_val(env);
    let topic =
        <soroban_sdk::xdr::ScVal as soroban_sdk::TryFromVal<Env, soroban_sdk::Val>>::try_from_val(
            env, &val,
        )
        .unwrap();
    env.events()
        .all()
        .filter_by_contract(contract)
        .events()
        .iter()
        .any(|e| match &e.body {
            soroban_sdk::xdr::ContractEventBody::V0(v0) => v0.topics.first() == Some(&topic),
        })
}

#[test]
fn test_merkle_tree_generation_and_proof_verification() {
    let env = Env::default();
    let (recipients, amounts, layers) = merkle_fixture(&env);
    let root = layers.last().unwrap()[0].clone();

    // Leaf hash is SHA256(recipient_xdr || amount_be), independently recomputed
    let mut preimage = soroban_sdk::xdr::ToXdr::to_xdr(recipients[0].clone(), &env);
    preimage.extend_from_array(&amounts[0].to_be_bytes());
    assert_eq!(
        merkle_leaf(&env, &recipients[0], amounts[0]),
        env.crypto().sha256(&preimage).to_bytes()
    );

    // Pair hashing is order-independent
    let (a, b) = (&layers[0][0], &layers[0][1]);
    assert_eq!(merkle_hash_pair(&env, a, b), merkle_hash_pair(&env, b, a));

    for i in 0..recipients.len() {
        let proof = merkle_proof(&env, &layers, i);
        let leaf = merkle_leaf(&env, &recipients[i], amounts[i]);
        assert!(verify_merkle_proof(&env, &proof, &root, &leaf));
    }
}

#[test]
fn test_merkle_payroll_locks_funds_and_stores_root() {
    let env = Env::default();
    let (sender, _, token, contract_id) = payroll_setup(&env);
    let orbit = OrbitContractClient::new(&env, &contract_id);
    let token_client = TokenClient::new(&env, &token);
    let (_, amounts, layers) = merkle_fixture(&env);
    let root = layers.last().unwrap()[0].clone();
    let total: i128 = amounts.iter().sum();

    let id = orbit.create_merkle_payroll(&sender, &token, &root, &total, &3_600);

    assert_eq!(token_client.balance(&contract_id), total);
    assert_eq!(token_client.balance(&sender), 100_0000000 - total);
    let payroll = orbit.get_merkle_payroll(&id).unwrap();
    assert_eq!(payroll.merkle_root, root);
    assert_eq!(payroll.total_deposited, total);
    assert_eq!(payroll.claimed_amount, 0);
    assert_eq!(payroll.expiration_timestamp, 1_000 + 3_600);
    assert_eq!(payroll.sender, sender);

    // Ids are sequential
    assert_eq!(
        orbit.create_merkle_payroll(&sender, &token, &root, &1, &3_600),
        id + 1
    );
}

#[test]
fn test_merkle_payroll_valid_multi_leaf_claims() {
    let env = Env::default();
    let (sender, _, token, contract_id) = payroll_setup(&env);
    let orbit = OrbitContractClient::new(&env, &contract_id);
    let token_client = TokenClient::new(&env, &token);
    let (recipients, amounts, layers) = merkle_fixture(&env);
    let root = layers.last().unwrap()[0].clone();
    let total: i128 = amounts.iter().sum();
    let id = orbit.create_merkle_payroll(&sender, &token, &root, &total, &3_600);

    for i in 0..recipients.len() {
        let proof = merkle_proof(&env, &layers, i);
        assert!(!orbit.is_merkle_leaf_claimed(&id, &recipients[i], &amounts[i]));
        orbit.claim_merkle_payroll(&id, &recipients[i], &amounts[i], &proof);
        assert_eq!(token_client.balance(&recipients[i]), amounts[i]);
        assert!(orbit.is_merkle_leaf_claimed(&id, &recipients[i], &amounts[i]));
    }

    let payroll = orbit.get_merkle_payroll(&id).unwrap();
    assert_eq!(payroll.claimed_amount, total);
    assert_eq!(token_client.balance(&contract_id), 0);
}

#[test]
fn test_revenue_pool_rejects_duplicate_members_wrong_token_and_outsider_claim() {
    let env = Env::default();
    env.mock_all_auths();
    let owner = Address::generate(&env);
    let shareholder = Address::generate(&env);
    let outsider = Address::generate(&env);
    let token = Address::generate(&env);
    let wrong_token = Address::generate(&env);
    let contract_id = env.register(OrbitContract, ());
    let orbit = OrbitContractClient::new(&env, &contract_id);

    let duplicates = soroban_sdk::vec![
        &env,
        (shareholder.clone(), 1u32),
        (shareholder.clone(), 2u32),
    ];
    assert!(orbit
        .try_create_revenue_pool(&owner, &1, &token, &duplicates)
        .is_err());

    let shareholders = soroban_sdk::vec![&env, (shareholder.clone(), 1u32)];
    orbit.create_revenue_pool(&owner, &2, &token, &shareholders);
    assert!(orbit
        .try_deposit_to_pool(&owner, &2, &wrong_token, &10)
        .is_err());
    assert!(orbit
        .try_claim_shareholder_dividends(&outsider, &2)
        .is_err());
}

// ----------------------------------------------------------------------------
// #128: DESIGNATED 14-DAY VAULT RECOVERY
// ----------------------------------------------------------------------------

#[test]
fn test_vault_recovery_timelock_veto_and_migration() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);
    let user = Address::generate(&env);
    let recovery = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = TokenClient::new(&env, &token);
    StellarAssetClient::new(&env, &token).mint(&user, &1_000);
    StellarAssetClient::new(&env, &token).mint(&recovery, &1_000);
    let contract_id = env.register(OrbitContract, ());
    let orbit = OrbitContractClient::new(&env, &contract_id);
    token_client.approve(&user, &contract_id, &1_000, &2_000_000);

    orbit.create_vault_with_recovery(&user, &merchant, &token, &100, &100, &recovery);
    orbit.initiate_vault_recovery(&recovery, &user, &merchant);
    let pending = orbit.get_vault_recovery(&user, &merchant).unwrap();
    assert_eq!(pending.initiated_at, 1_000);
    assert_eq!(pending.execute_after, 1_000 + RECOVERY_TIMELOCK_SECONDS);
    assert!(orbit.get_vault(&user, &merchant).unwrap().is_paused);
    assert!(orbit
        .try_execute_vault_recovery(&recovery, &user, &merchant)
        .is_err());
    assert!(orbit.try_pull_funds(&user, &merchant).is_err());

    // The original key vetoes immediately and restores the pre-request state.
    orbit.cancel_recovery(&user, &merchant);
    assert!(orbit.get_vault_recovery(&user, &merchant).is_none());
    assert!(!orbit.get_vault(&user, &merchant).unwrap().is_paused);

    orbit.initiate_vault_recovery(&recovery, &user, &merchant);
    env.ledger()
        .set_timestamp(1_000 + RECOVERY_TIMELOCK_SECONDS);
    orbit.execute_vault_recovery(&recovery, &user, &merchant);

    assert!(orbit.get_vault(&user, &merchant).is_none());
    let migrated = orbit.get_vault(&recovery, &merchant).unwrap();
    assert!(migrated.is_paused);
    assert_eq!(migrated.token, token);
    assert_eq!(migrated.amount_per_interval, 100);
    assert_eq!(migrated.interval_seconds, 100);
    assert!(orbit.try_pull_funds(&user, &merchant).is_err());

    // Fresh approval and an explicit resume by the recovered account are
    // required before the replacement vault can be billed.
    token_client.approve(&recovery, &contract_id, &1_000, &3_000_000);
    orbit.resume_vault(&recovery, &merchant);
    env.ledger()
        .set_timestamp(1_000 + RECOVERY_TIMELOCK_SECONDS + 100);
    orbit.pull_funds(&recovery, &merchant);
    assert_eq!(token_client.balance(&merchant), 100);
}

#[test]
fn test_only_designated_address_can_initiate_recovery() {
    let env = Env::default();
    env.mock_all_auths();
    let user = Address::generate(&env);
    let recovery = Address::generate(&env);
    let impostor = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token = Address::generate(&env);
    let contract_id = env.register(OrbitContract, ());
    let orbit = OrbitContractClient::new(&env, &contract_id);
    orbit.create_vault_with_recovery(&user, &merchant, &token, &100, &100, &recovery);

    assert!(orbit
        .try_initiate_vault_recovery(&impostor, &user, &merchant)
        .is_err());
    assert!(orbit.get_vault_recovery(&user, &merchant).is_none());
}

// ----------------------------------------------------------------------------
// #129: CAPPED INTERVAL ELASTICITY WITH CARRY-FORWARD
// ----------------------------------------------------------------------------

#[test]
fn test_elastic_metered_overdraft_carries_and_rebalances() {
    let env = Env::default();
    env.mock_all_auths();
    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = TokenClient::new(&env, &token);
    StellarAssetClient::new(&env, &token).mint(&user, &1_000);
    let contract_id = env.register(OrbitContract, ());
    let orbit = OrbitContractClient::new(&env, &contract_id);
    token_client.approve(&user, &contract_id, &1_000, &2_000_000);

    orbit.create_elastic_metered_vault(&user, &merchant, &token, &100, &100, &20);
    orbit.pull_metered_funds(&user, &merchant, &120);
    assert_eq!(
        orbit
            .get_metered_elasticity(&user, &merchant)
            .unwrap()
            .epoch_overdraft,
        20
    );
    assert!(orbit.try_pull_metered_funds(&user, &merchant, &1).is_err());

    // The prior 20-unit overdraft reduces the next baseline from 100 to 80;
    // with the 20-unit buffer, that cycle's combined cap is 100.
    env.ledger().set_timestamp(100);
    orbit.pull_metered_funds(&user, &merchant, &100);
    let second = orbit.get_metered_elasticity(&user, &merchant).unwrap();
    assert_eq!(second.carried_overdraft, 20);
    assert_eq!(second.epoch_overdraft, 20);
    assert!(orbit.try_pull_metered_funds(&user, &merchant, &1).is_err());

    // Spending only the adjusted 80-unit baseline clears new overdraft usage.
    env.ledger().set_timestamp(200);
    orbit.pull_metered_funds(&user, &merchant, &80);
    let third = orbit.get_metered_elasticity(&user, &merchant).unwrap();
    assert_eq!(third.carried_overdraft, 20);
    assert_eq!(third.epoch_overdraft, 0);

    // The following cycle is fully replenished and can again use 100 + 20%.
    env.ledger().set_timestamp(300);
    orbit.pull_metered_funds(&user, &merchant, &120);
    let fourth = orbit.get_metered_elasticity(&user, &merchant).unwrap();
    assert_eq!(fourth.carried_overdraft, 0);
    assert_eq!(fourth.epoch_overdraft, 20);
    assert_eq!(token_client.balance(&merchant), 420);
}

#[test]
fn test_elastic_metered_rejects_overdraft_above_twenty_percent() {
    let env = Env::default();
    env.mock_all_auths();
    let user = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token = Address::generate(&env);
    let contract_id = env.register(OrbitContract, ());
    let orbit = OrbitContractClient::new(&env, &contract_id);
    assert!(orbit
        .try_create_elastic_metered_vault(&user, &merchant, &token, &100, &100, &21)
        .is_err());
    assert!(orbit.get_metered_vault(&user, &merchant).is_none());
}

// ----------------------------------------------------------------------------
// #130: MERCHANT GAS-SUBSIDY RESERVE & SPONSORED HANDSHAKE
// ----------------------------------------------------------------------------

#[test]
fn test_gas_subsidy_deposit_sponsored_onboarding_and_withdrawal() {
    let env = Env::default();
    env.mock_all_auths();
    let merchant = Address::generate(&env);
    let sponsor = Address::generate(&env);
    let user = Address::generate(&env);
    let native_admin = Address::generate(&env);
    let native_token = env
        .register_stellar_asset_contract_v2(native_admin.clone())
        .address();
    let subscription_token = Address::generate(&env);
    let native_client = TokenClient::new(&env, &native_token);
    StellarAssetClient::new(&env, &native_token).mint(&merchant, &1_000);
    let contract_id = env.register(OrbitContract, ());
    let orbit = OrbitContractClient::new(&env, &contract_id);
    native_client.approve(&merchant, &contract_id, &1_000, &2_000_000);

    orbit.configure_gas_subsidy(&merchant, &native_token, &sponsor, &50);
    orbit.deposit_gas_subsidy(&merchant, &500);
    assert_eq!(orbit.get_gas_subsidy_pool(&merchant).unwrap().balance, 500);
    assert_eq!(native_client.balance(&contract_id), 500);

    orbit.withdraw_gas_subsidy(&merchant, &100);
    assert_eq!(orbit.get_gas_subsidy_pool(&merchant).unwrap().balance, 400);
    assert_eq!(native_client.balance(&merchant), 600);

    // The subscriber holds no native token. In production the configured
    // sponsor is the transaction source; this invocation only reimburses it.
    assert_eq!(native_client.balance(&user), 0);
    orbit.create_sponsored_vault(&user, &merchant, &subscription_token, &25, &100, &40, &0);
    assert!(orbit.get_vault(&user, &merchant).is_some());
    assert_eq!(native_client.balance(&sponsor), 40);
    let pool = orbit.get_gas_subsidy_pool(&merchant).unwrap();
    assert_eq!(pool.balance, 360);
    assert_eq!(pool.next_nonce, 1);
    assert!(!pool.locked);

    orbit.withdraw_gas_subsidy(&merchant, &360);
    assert_eq!(orbit.get_gas_subsidy_pool(&merchant).unwrap().balance, 0);
    assert_eq!(native_client.balance(&contract_id), 0);
}

#[test]
fn test_gas_subsidy_caps_and_nonce_prevent_reserve_drain() {
    let env = Env::default();
    env.mock_all_auths();
    let merchant = Address::generate(&env);
    let sponsor = Address::generate(&env);
    let user = Address::generate(&env);
    let native_admin = Address::generate(&env);
    let native_token = env
        .register_stellar_asset_contract_v2(native_admin.clone())
        .address();
    let subscription_token = Address::generate(&env);
    StellarAssetClient::new(&env, &native_token).mint(&merchant, &100);
    let contract_id = env.register(OrbitContract, ());
    let native_client = TokenClient::new(&env, &native_token);
    let orbit = OrbitContractClient::new(&env, &contract_id);
    native_client.approve(&merchant, &contract_id, &100, &2_000_000);
    orbit.configure_gas_subsidy(&merchant, &native_token, &sponsor, &10);
    orbit.deposit_gas_subsidy(&merchant, &100);

    assert!(orbit
        .try_create_sponsored_vault(&user, &merchant, &subscription_token, &25, &100, &11, &0,)
        .is_err());
    assert_eq!(orbit.get_gas_subsidy_pool(&merchant).unwrap().balance, 100);

    orbit.create_sponsored_vault(&user, &merchant, &subscription_token, &25, &100, &10, &0);
    assert!(orbit
        .try_create_sponsored_vault(&user, &merchant, &subscription_token, &25, &100, &10, &0,)
        .is_err());
    let pool = orbit.get_gas_subsidy_pool(&merchant).unwrap();
    assert_eq!(pool.balance, 90);
    assert_eq!(pool.next_nonce, 1);
    assert_eq!(native_client.balance(&sponsor), 10);
}

#[test]
fn test_merkle_payroll_emits_created_and_claimed_events() {
    let env = Env::default();
    let (sender, _, token, contract_id) = payroll_setup(&env);
    let orbit = OrbitContractClient::new(&env, &contract_id);
    let (recipients, amounts, layers) = merkle_fixture(&env);
    let root = layers.last().unwrap()[0].clone();
    let id = orbit.create_merkle_payroll(&sender, &token, &root, &100_0000000, &3_600);
    assert!(has_event(&env, &contract_id, "merkle_payroll_created"));

    orbit.claim_merkle_payroll(
        &id,
        &recipients[1],
        &amounts[1],
        &merkle_proof(&env, &layers, 1),
    );
    assert!(has_event(&env, &contract_id, "merkle_payroll_claimed"));
}

#[test]
fn test_merkle_payroll_rejects_double_claim() {
    let env = Env::default();
    let (sender, _, token, contract_id) = payroll_setup(&env);
    let orbit = OrbitContractClient::new(&env, &contract_id);
    let (recipients, amounts, layers) = merkle_fixture(&env);
    let root = layers.last().unwrap()[0].clone();
    let id = orbit.create_merkle_payroll(&sender, &token, &root, &100_0000000, &3_600);

    let proof = merkle_proof(&env, &layers, 2);
    orbit.claim_merkle_payroll(&id, &recipients[2], &amounts[2], &proof);
    assert!(orbit
        .try_claim_merkle_payroll(&id, &recipients[2], &amounts[2], &proof)
        .is_err());
    assert_eq!(
        TokenClient::new(&env, &token).balance(&recipients[2]),
        amounts[2]
    );
}

#[test]
fn test_merkle_payroll_rejects_tampered_proofs_and_leaves() {
    let env = Env::default();
    let (sender, _, token, contract_id) = payroll_setup(&env);
    let orbit = OrbitContractClient::new(&env, &contract_id);
    let token_client = TokenClient::new(&env, &token);
    let (recipients, amounts, layers) = merkle_fixture(&env);
    let root = layers.last().unwrap()[0].clone();
    let id = orbit.create_merkle_payroll(&sender, &token, &root, &100_0000000, &3_600);
    let proof = merkle_proof(&env, &layers, 0);

    // Inflated amount for a real recipient
    assert!(orbit
        .try_claim_merkle_payroll(&id, &recipients[0], &(amounts[0] + 1), &proof)
        .is_err());
    // Another address replaying a valid proof
    let attacker = Address::generate(&env);
    assert!(orbit
        .try_claim_merkle_payroll(&id, &attacker, &amounts[0], &proof)
        .is_err());
    // A valid leaf with someone else's proof
    assert!(orbit
        .try_claim_merkle_payroll(&id, &recipients[1], &amounts[1], &proof)
        .is_err());
    // A flipped sibling hash
    let mut tampered = proof.clone();
    let mut bytes = tampered.get(0).unwrap().to_array();
    bytes[0] ^= 0xff;
    tampered.set(0, BytesN::from_array(&env, &bytes));
    assert!(orbit
        .try_claim_merkle_payroll(&id, &recipients[0], &amounts[0], &tampered)
        .is_err());
    // An empty proof
    assert!(orbit
        .try_claim_merkle_payroll(&id, &recipients[0], &amounts[0], &Vec::new(&env))
        .is_err());

    assert_eq!(token_client.balance(&attacker), 0);
    assert_eq!(orbit.get_merkle_payroll(&id).unwrap().claimed_amount, 0);
}

#[test]
#[should_panic(expected = "Invalid Merkle proof")]
fn test_merkle_payroll_invalid_proof_panics() {
    let env = Env::default();
    let (sender, _, token, contract_id) = payroll_setup(&env);
    let orbit = OrbitContractClient::new(&env, &contract_id);
    let (recipients, amounts, layers) = merkle_fixture(&env);
    let root = layers.last().unwrap()[0].clone();
    let id = orbit.create_merkle_payroll(&sender, &token, &root, &100_0000000, &3_600);
    orbit.claim_merkle_payroll(
        &id,
        &recipients[0],
        &(amounts[0] * 2),
        &merkle_proof(&env, &layers, 0),
    );
}

#[test]
fn test_merkle_payroll_cannot_claim_more_than_deposited() {
    let env = Env::default();
    let (sender, _, token, contract_id) = payroll_setup(&env);
    let orbit = OrbitContractClient::new(&env, &contract_id);
    let (recipients, amounts, layers) = merkle_fixture(&env);
    let root = layers.last().unwrap()[0].clone();
    // Underfunded: only the first allocation is covered
    let id = orbit.create_merkle_payroll(&sender, &token, &root, &amounts[0], &3_600);

    orbit.claim_merkle_payroll(
        &id,
        &recipients[0],
        &amounts[0],
        &merkle_proof(&env, &layers, 0),
    );
    assert!(orbit
        .try_claim_merkle_payroll(
            &id,
            &recipients[1],
            &amounts[1],
            &merkle_proof(&env, &layers, 1)
        )
        .is_err());
}

#[test]
fn test_merkle_payroll_expiry_and_reclaim() {
    let env = Env::default();
    let (sender, _, token, contract_id) = payroll_setup(&env);
    let orbit = OrbitContractClient::new(&env, &contract_id);
    let token_client = TokenClient::new(&env, &token);
    let (recipients, amounts, layers) = merkle_fixture(&env);
    let root = layers.last().unwrap()[0].clone();
    let total: i128 = amounts.iter().sum();
    let id = orbit.create_merkle_payroll(&sender, &token, &root, &total, &1_000);

    orbit.claim_merkle_payroll(
        &id,
        &recipients[0],
        &amounts[0],
        &merkle_proof(&env, &layers, 0),
    );
    assert!(orbit.try_reclaim_merkle_payroll(&sender, &id).is_err());

    env.ledger().with_mut(|l| l.timestamp = 2_001);
    assert!(orbit
        .try_claim_merkle_payroll(
            &id,
            &recipients[1],
            &amounts[1],
            &merkle_proof(&env, &layers, 1)
        )
        .is_err());
    assert!(orbit
        .try_reclaim_merkle_payroll(&Address::generate(&env), &id)
        .is_err());

    assert_eq!(
        orbit.reclaim_merkle_payroll(&sender, &id),
        total - amounts[0]
    );
    assert_eq!(token_client.balance(&sender), 100_0000000 - amounts[0]);
    assert!(orbit.try_reclaim_merkle_payroll(&sender, &id).is_err());
}

#[test]
fn test_merkle_payroll_requires_sender_auth() {
    let env = Env::default();
    let (sender, _, token, contract_id) = payroll_setup(&env);
    let orbit = OrbitContractClient::new(&env, &contract_id);
    let root = BytesN::from_array(&env, &[7u8; 32]);
    env.set_auths(&[]);
    assert!(orbit
        .try_create_merkle_payroll(&sender, &token, &root, &1_0000000, &3_600)
        .is_err());
}

// ----------------------------------------------------------------------------
// YIELD-BEARING COLLATERAL STRATEGY TESTS (#176)
// ----------------------------------------------------------------------------

// Mock lending pool contract for testing
#[contract]
pub struct MockLendingPool;

#[contractimpl]
impl MockLendingPool {
    pub fn supply(_env: Env, _token: Address, _amount: i128, _from: Address) {
        // Mock supply - funds are tracked by the contract itself
    }

    pub fn withdraw(_env: Env, _token: Address, amount: i128, _to: Address) -> i128 {
        // Mock withdraw - return slightly more than requested (simulating yield)
        amount + (amount / 100) // 1% yield
    }

    pub fn balance_of(_env: Env, _token: Address, _account: Address) -> i128 {
        1000_0000000 // Mock balance
    }
}

#[test]
fn test_configure_yield_strategy() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let token_address = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();

    let pool_id = env.register(MockLendingPool, ());

    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    orbit_client.configure_yield_strategy(
        &admin,
        &pool_id,
        &token_address,
        &10_0000000, // 10 token liquidity buffer
    );

    let config = orbit_client.get_yield_strategy();
    assert!(config.is_some());
    let config = config.unwrap();
    assert_eq!(config.token, token_address);
    assert_eq!(config.liquidity_buffer, 10_0000000);
    assert_eq!(config.total_deployed, 0);
}

#[test]
fn test_deploy_idle_capital() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let token_address = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = TokenClient::new(&env, &token_address);
    let token_admin_client = StellarAssetClient::new(&env, &token_address);

    let pool_id = env.register(MockLendingPool, ());
    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    // Configure yield strategy with 10 token buffer
    orbit_client.configure_yield_strategy(&admin, &pool_id, &token_address, &10_0000000);

    // Mint 50 tokens to the contract (simulating escrow deposits)
    token_admin_client.mint(&contract_id, &50_0000000);

    // Deploy excess capital (50 - 10 = 40 should be deployed)
    orbit_client.deploy_idle_capital(&token_address);

    let config = orbit_client.get_yield_strategy().unwrap();
    assert_eq!(config.total_deployed, 40_0000000);
}

#[test]
fn test_redeem_yield_principal() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let token_address = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_admin_client = StellarAssetClient::new(&env, &token_address);

    let pool_id = env.register(MockLendingPool, ());
    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    orbit_client.configure_yield_strategy(&admin, &pool_id, &token_address, &10_0000000);
    token_admin_client.mint(&contract_id, &50_0000000);
    orbit_client.deploy_idle_capital(&token_address);

    // Redeem 20 tokens (will get 20 + 1% = 20.2 due to mock yield)
    orbit_client.redeem_yield_principal(&token_address, &20_0000000);

    let config = orbit_client.get_yield_strategy().unwrap();
    assert_eq!(config.total_deployed, 20_0000000); // 40 - 20 = 20 remaining deployed
}

#[test]
fn test_emergency_unwind_yield() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let token_address = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_admin_client = StellarAssetClient::new(&env, &token_address);

    let pool_id = env.register(MockLendingPool, ());
    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    orbit_client.configure_yield_strategy(&admin, &pool_id, &token_address, &10_0000000);
    token_admin_client.mint(&contract_id, &50_0000000);
    orbit_client.deploy_idle_capital(&token_address);

    // Emergency unwind all funds
    orbit_client.emergency_unwind_yield(&admin);

    let config = orbit_client.get_yield_strategy().unwrap();
    assert_eq!(config.total_deployed, 0);
}

#[test]
#[should_panic(expected = "No excess capital to deploy")]
fn test_deploy_fails_when_no_excess() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let token_address = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_admin_client = StellarAssetClient::new(&env, &token_address);

    let pool_id = env.register(MockLendingPool, ());
    let contract_id = env.register(OrbitContract, ());
    let orbit_client = OrbitContractClient::new(&env, &contract_id);

    orbit_client.configure_yield_strategy(&admin, &pool_id, &token_address, &50_0000000);
    // Mint exactly the buffer amount - no excess to deploy
    token_admin_client.mint(&contract_id, &50_0000000);

    orbit_client.deploy_idle_capital(&token_address);
}


// ----------------------------------------------------------------------------
// ISSUE #212: COMPETITIVE BOUNTY COMMIT-REVEAL
// ----------------------------------------------------------------------------

struct BountyCase {
    env: Env,
    contract: Address,
    token: Address,
    creator: Address,
    claimant: Address,
    proof: Bytes,
    salt: BytesN<32>,
    bounty_id: u64,
}

impl BountyCase {
    fn new() -> Self {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_sequence_number(100);
        let creator = Address::generate(&env);
        let claimant = Address::generate(&env);
        let admin = Address::generate(&env);
        let token = env.register_stellar_asset_contract_v2(admin.clone()).address();
        StellarAssetClient::new(&env, &token).mint(&creator, &1_000_000_000);
        let contract = env.register(OrbitContract, ());
        let client = OrbitContractClient::new(&env, &contract);
        let proof = Bytes::from_slice(&env, b"reviewed-deliverable-49");
        let salt = BytesN::from_array(&env, &[7u8; 32]);
        let proof_hash = env.crypto().sha256(&proof).to_bytes();
        let bounty_id = client.create_bounty(&creator, &token, &500_000_000, &proof_hash, &120);
        Self { env, contract, token, creator, claimant, proof, salt, bounty_id }
    }

    fn commit(&self) {
        let digest = bounty_claim_digest(&self.env, &self.claimant, self.bounty_id, &self.proof, &self.salt);
        OrbitContractClient::new(&self.env, &self.contract)
            .commit_bounty_claim(&self.claimant, &self.bounty_id, &digest);
    }

    fn reveal(&self) {
        OrbitContractClient::new(&self.env, &self.contract)
            .reveal_bounty_claim(&self.claimant, &self.bounty_id, &self.proof, &self.salt);
    }
}

#[test]
fn test_bounty_commit_reveal_disburses_once() {
    let c = BountyCase::new();
    let token = TokenClient::new(&c.env, &c.token);
    assert_eq!(token.balance(&c.creator), 500_000_000);
    assert_eq!(token.balance(&c.contract), 500_000_000);
    c.commit();
    c.env.ledger().set_sequence_number(105);
    c.reveal();
    assert_eq!(token.balance(&c.claimant), 500_000_000);
    assert_eq!(token.balance(&c.contract), 0);
    let bounty = OrbitContractClient::new(&c.env, &c.contract).get_bounty(&c.bounty_id).unwrap();
    assert_eq!(bounty.claimed_by, Some(c.claimant.clone()));
    assert!(OrbitContractClient::new(&c.env, &c.contract)
        .try_reveal_bounty_claim(&c.claimant, &c.bounty_id, &c.proof, &c.salt).is_err());
}

#[test]
fn test_bounty_early_reveal_rejected() {
    let c = BountyCase::new();
    c.commit();
    c.env.ledger().set_sequence_number(104);
    assert!(OrbitContractClient::new(&c.env, &c.contract)
        .try_reveal_bounty_claim(&c.claimant, &c.bounty_id, &c.proof, &c.salt).is_err());
    c.env.ledger().set_sequence_number(105);
    c.reveal();
}

#[test]
fn test_bounty_tampered_salt_and_proof_rejected() {
    let c = BountyCase::new();
    c.commit();
    c.env.ledger().set_sequence_number(105);
    let wrong_salt = BytesN::from_array(&c.env, &[8u8; 32]);
    let wrong_proof = Bytes::from_slice(&c.env, b"stolen-deliverable");
    let client = OrbitContractClient::new(&c.env, &c.contract);
    assert!(client.try_reveal_bounty_claim(&c.claimant, &c.bounty_id, &c.proof, &wrong_salt).is_err());
    assert!(client.try_reveal_bounty_claim(&c.claimant, &c.bounty_id, &wrong_proof, &c.salt).is_err());
    assert_eq!(TokenClient::new(&c.env, &c.token).balance(&c.contract), 500_000_000);
    c.reveal();
}

#[test]
fn test_bounty_mismatched_claimant_cannot_steal_commitment() {
    let c = BountyCase::new();
    c.commit();
    c.env.ledger().set_sequence_number(105);
    let thief = Address::generate(&c.env);
    let client = OrbitContractClient::new(&c.env, &c.contract);
    assert!(client.try_reveal_bounty_claim(&thief, &c.bounty_id, &c.proof, &c.salt).is_err());
    // A different claimant cannot reuse the digest even after committing it.
    let digest = bounty_claim_digest(&c.env, &c.claimant, c.bounty_id, &c.proof, &c.salt);
    client.commit_bounty_claim(&thief, &c.bounty_id, &digest);
    c.env.ledger().set_sequence_number(110);
    assert!(client.try_reveal_bounty_claim(&thief, &c.bounty_id, &c.proof, &c.salt).is_err());
    c.reveal();
}

#[test]
fn test_bounty_expiry_refund_cannot_be_double_spent() {
    let c = BountyCase::new();
    c.env.ledger().set_sequence_number(121);
    let client = OrbitContractClient::new(&c.env, &c.contract);
    client.reclaim_expired_bounty(&c.bounty_id);
    assert_eq!(TokenClient::new(&c.env, &c.token).balance(&c.creator), 1_000_000_000);
    assert!(client.try_reclaim_expired_bounty(&c.bounty_id).is_err());
    assert!(client.try_commit_bounty_claim(&c.claimant, &c.bounty_id, &BytesN::from_array(&c.env, &[0u8; 32])).is_err());
}


// ----------------------------------------------------------------------------
// ISSUE #209: FUNDED MILESTONE ESCROW AND MERCHANT SIGN-OFF
// ----------------------------------------------------------------------------

struct MilestoneCase {
    env: Env,
    client_address: Address,
    creator: Address,
    contractor: Address,
    token: Address,
    id: u64,
}

impl MilestoneCase {
    fn new() -> Self {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(100);
        let creator = Address::generate(&env);
        let contractor = Address::generate(&env);
        let admin = Address::generate(&env);
        let token = env.register_stellar_asset_contract_v2(admin.clone()).address();
        StellarAssetClient::new(&env, &token).mint(&creator, &100_0000000);
        let client_address = env.register(OrbitContract, ());
        let client = OrbitContractClient::new(&env, &client_address);
        let first = MilestoneInput {
            title: String::from_str(&env, "Design"),
            deliverable_url: String::from_str(&env, "https://example.com/spec"),
            amount: 10_0000000,
            due_timestamp: 500,
        };
        let second = MilestoneInput {
            title: String::from_str(&env, "Implementation"),
            deliverable_url: String::from_str(&env, "https://example.com/code"),
            amount: 20_0000000,
            due_timestamp: 600,
        };
        let stages = soroban_sdk::vec![&env, first, second];
        let id = client.create_milestone_project(&creator, &contractor, &token, &stages);
        Self { env, client_address, creator, contractor, token, id }
    }
    fn submit(&self, stage: u32) {
        OrbitContractClient::new(&self.env, &self.client_address)
            .submit_milestone_deliverable(
                &self.contractor, &self.id, &stage,
                &String::from_str(&self.env, "https://example.com/proof")
            );
    }
}

#[test]
fn test_milestone_escrow_requires_merchant_signoff() {
    let c = MilestoneCase::new();
    let client = OrbitContractClient::new(&c.env, &c.client_address);
    let token = TokenClient::new(&c.env, &c.token);
    assert_eq!(token.balance(&c.creator), 70_0000000);
    assert_eq!(token.balance(&c.client_address), 30_0000000);
    assert!(client.try_approve_and_release_milestone(&c.creator, &c.id, &0).is_err());
    c.submit(0);
    let stage = client.get_milestone_project(&c.id).unwrap().stages.get(0).unwrap();
    assert!(stage.submitted);
    assert!(!stage.released);
    client.approve_and_release_milestone(&c.creator, &c.id, &0);
    assert_eq!(token.balance(&c.contractor), 10_0000000);
    assert_eq!(token.balance(&c.client_address), 20_0000000);
    assert!(client.try_approve_and_release_milestone(&c.creator, &c.id, &0).is_err());
}

#[test]
fn test_milestone_changes_force_new_submission_before_release() {
    let c = MilestoneCase::new();
    let client = OrbitContractClient::new(&c.env, &c.client_address);
    c.submit(0);
    client.request_milestone_changes(&c.creator, &c.id, &0,
        &String::from_str(&c.env, "Please include unit tests"));
    assert!(client.try_approve_and_release_milestone(&c.creator, &c.id, &0).is_err());
    assert!(!client.get_milestone_project(&c.id).unwrap().stages.get(0).unwrap().submitted);
    c.submit(0);
    client.approve_and_release_milestone(&c.creator, &c.id, &0);
}

#[test]
fn test_milestone_cannot_skip_earlier_approval() {
    let c = MilestoneCase::new();
    let client = OrbitContractClient::new(&c.env, &c.client_address);
    c.submit(1);
    assert!(client.try_approve_and_release_milestone(&c.creator, &c.id, &1).is_err());
    c.submit(0);
    client.approve_and_release_milestone(&c.creator, &c.id, &0);
    client.approve_and_release_milestone(&c.creator, &c.id, &1);
    assert_eq!(client.get_milestone_project(&c.id).unwrap().released_amount, 30_0000000);
}

#[test]
fn test_milestone_wrong_roles_cannot_approve_or_submit() {
    let c = MilestoneCase::new();
    let client = OrbitContractClient::new(&c.env, &c.client_address);
    let outsider = Address::generate(&c.env);
    assert!(client.try_submit_milestone_deliverable(
        &outsider, &c.id, &0, &String::from_str(&c.env, "https://example.com/false")).is_err());
    c.submit(0);
    assert!(client.try_request_milestone_changes(
        &outsider, &c.id, &0, &String::from_str(&c.env, "bad")).is_err());
    assert!(client.try_approve_and_release_milestone(&outsider, &c.id, &0).is_err());
    client.approve_and_release_milestone(&c.creator, &c.id, &0);
}
