#![no_std]
#![allow(deprecated)]
#![allow(clippy::too_many_arguments)]

use soroban_sdk::{
    contract, contractclient, contractimpl, contracttype, token, Address, Bytes, BytesN, Env,
    String, Symbol, Vec,
};

// ----------------------------------------------------------------------------
// CONSTANTS: STATE ARCHIVAL & TTL MANAGEMENT
// ----------------------------------------------------------------------------

/// Approximate duration of one Stellar ledger in seconds (~5 seconds per ledger).
pub const SECONDS_PER_LEDGER: u64 = 5;

/// Safety margin added to interval ledgers (approx. 7 days = 120,960 ledgers)
/// to ensure vaults remain active even if automated pulls are delayed.
pub const TTL_MARGIN_LEDGERS: u32 = 120_960;

/// Base minimum TTL duration in ledgers (approx. 30 days = 518,400 ledgers).
pub const MIN_TTL_LEDGERS: u32 = 518_400;

/// Threshold in ledgers below which TTL will be extended (approx. 7 days = 120,960 ledgers).
pub const TTL_THRESHOLD_LEDGERS: u32 = 120_960;
pub const DEFAULT_GRACE_PERIOD_SECONDS: u64 = 3 * 24 * 60 * 60;
pub const PAYMENT_ACTIVE: u32 = 0;
pub const PAYMENT_GRACE_PERIOD: u32 = 1;
pub const PAYMENT_DEFAULTED: u32 = 2;

/// Sentinel `pull_sequence` for a vault that has never settled a pull.
/// Lets the very first pull through while every later pull must advance the
/// monotonic billing epoch (`cycle_index > pull_sequence`).
pub const PULL_SEQUENCE_UNSET: u64 = u64::MAX;

/// Upper bound on a payroll proposal's approver list, keeping approval checks and
/// proposal storage within Soroban resource limits.
pub const MAX_PAYROLL_APPROVERS: u32 = 20;

/// Upper bound on a `batch_pull_funds` subscriber list (#92). Caps the ledger
/// work of a single transaction so a merchant with hundreds of due subscribers
/// can page through them in bounded batches.
pub const MAX_BATCH_PULL_SUBSCRIBERS: u32 = 50;

/// Fixed-point precision used by revenue pools when accumulating dividends.
pub const REVENUE_ACC_SCALE: i128 = 1_000_000_000_000;

/// Keeps revenue-pool creation and claim lookups within predictable resource bounds.
pub const MAX_REVENUE_POOL_SHAREHOLDERS: u32 = 100;

/// Maximum elastic allowance above a metered vault's baseline limit.
pub const MAX_OVERDRAFT_PERCENTAGE: u32 = 20;

/// Challenge period between recovery initiation and execution.
pub const RECOVERY_TIMELOCK_SECONDS: u64 = 14 * 24 * 60 * 60;

/// Calculates (threshold, extend_to) in ledgers for vault persistent storage
/// and contract instance storage based on interval_seconds.
pub fn calculate_ttl_params(interval_seconds: u64) -> (u32, u32) {
    let interval_ledgers = (interval_seconds / SECONDS_PER_LEDGER) as u32;
    let extend_to = interval_ledgers
        .saturating_add(TTL_MARGIN_LEDGERS)
        .max(MIN_TTL_LEDGERS);
    let threshold = TTL_THRESHOLD_LEDGERS.min(extend_to.saturating_sub(1));
    (threshold, extend_to)
}

// ----------------------------------------------------------------------------
// DATA STRUCTURES
// ----------------------------------------------------------------------------

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VaultKey {
    pub user: Address,
    pub merchant: Address,
}

/// Vault terms as returned by `get_vault`.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VaultData {
    pub token: Address,
    pub amount_per_interval: i128,
    pub interval_seconds: u64,
    pub last_pull_timestamp: u64,
    pub grace_period_seconds: u64,
    pub payment_status: u32,
    /// Ledger timestamp the vault was (last) created at; anchors the billing
    /// cycle grid used by the monotonic pull epoch guard (#94).
    pub initial_timestamp: u64,
    /// Cycle index of the most recent settlement. `PULL_SEQUENCE_UNSET` until
    /// the first successful pull (#94).
    pub pull_sequence: u64,
    /// Subscriber-controlled freeze flag; blocks `pull_funds` while true (#93).
    pub is_paused: bool,
    /// Timestamp after which merchant pulls are allowed. Zero means the vault
    /// has no trial window and billing starts immediately.
    pub trial_end_timestamp: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamKey {
    pub id: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamData {
    pub sender: Address,
    pub recipient: Address,
    pub token: Address,
    pub deposit_amount: i128,
    pub start_time: u64,
    pub stop_time: u64,
    pub amount_withdrawn: i128,
}

#[contracttype]
pub enum StorageKey {
    NextStreamId,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaymentSplit {
    pub recipient: Address,
    pub amount: i128,
}

/// Per-subscriber outcome of a `batch_pull_funds` call (#92). `success` is false
/// when the subscriber was skipped (missing/inactive vault, not yet due, already
/// settled this cycle, or insufficient funds) without reverting the batch.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatchPullResult {
    pub subscriber: Address,
    pub success: bool,
    /// Amount settled for this subscriber in raw token units; 0 when skipped.
    pub amount: i128,
}

/// A batch disbursement that only executes once `required_approvals` distinct
/// addresses from `approvers` have approved it.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PayrollProposal {
    pub id: u64,
    pub sender: Address,
    pub token: Address,
    pub splits: Vec<PaymentSplit>,
    pub approvers: Vec<Address>,
    pub approvals: Vec<Address>,
    pub required_approvals: u32,
    pub executed: bool,
}

/// A payroll deposit held in contract escrow until the recipient claims it or the
/// sender reclaims what is left after `expiration_timestamp` (#188).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PayrollEscrow {
    pub id: u64,
    pub sender: Address,
    pub recipient: Address,
    pub token: Address,
    pub total_deposited: i128,
    pub claimed_amount: i128,
    pub expiration_timestamp: u64,
    /// Set once the sender reclaims the unclaimed balance. Blocks any further
    /// claim or reclaim, so the remaining funds can never be withdrawn twice.
    pub reclaimed: bool,
}

/// A Merkle payroll distribution (#156): the sender escrows the full payroll
/// once and publishes only the Merkle root of every `(recipient, amount)`
/// allocation. Recipients then claim individually with a proof, so payroll for
/// thousands of contractors never has to fit in a single transaction.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MerklePayrollData {
    pub sender: Address,
    pub token: Address,
    pub merkle_root: BytesN<32>,
    pub total_deposited: i128,
    pub claimed_amount: i128,
    pub expiration_timestamp: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    /// Id the next payroll proposal will receive
    NextProposalId,
    Proposal(u64),
    /// Id the next payroll escrow deposit will receive
    NextPayrollEscrowId,
    PayrollEscrow(u64),
    /// Id the next Merkle payroll distribution will receive (#156)
    NextMerklePayrollId,
    MerklePayroll(u64),
    /// Claimed flag for one leaf of a Merkle payroll: (claim_id, leaf hash)
    MerkleClaimed(u64, BytesN<32>),
    /// Router a merchant has pre-approved for cross-asset subscriptions (#191)
    TrustedSwapRouter(Address, Address),
    NextBountyId,
    Bounty(u64),
    BountyCommit(u64, Address),
    NextMilestoneProjectId,
    MilestoneProject(u64),
}

/// Escrowed competitive bounty; creator cannot withdraw while it is claimable.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompetitiveBounty {
    pub creator: Address,
    pub token: Address,
    pub amount: i128,
    pub proof_hash: BytesN<32>,
    pub expires_at_ledger: u32,
    pub claimed_by: Option<Address>,
    pub cancelled: bool,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BountyCommitment {
    pub digest: BytesN<32>,
    pub ledger: u32,
}

/// The commitment binds claimant and bounty ID, so copied reveals cannot steal rewards.
/// Preimage = domain || claimant ScVal XDR || bounty ID BE || proof length BE || proof || salt.
pub fn bounty_claim_digest(
    env: &Env,
    claimant: &Address,
    bounty_id: u64,
    proof: &Bytes,
    salt: &BytesN<32>,
) -> BytesN<32> {
    use soroban_sdk::xdr::ToXdr;
    assert!(proof.len() > 0 && proof.len() <= 4096, "invalid proof length");
    let mut data = Bytes::from_slice(env, b"orbit:bounty:claim:v1");
    data.append(&claimant.clone().to_xdr(env));
    data.extend_from_array(&bounty_id.to_be_bytes());
    data.extend_from_array(&proof.len().to_be_bytes());
    data.append(proof);
    data.extend_from_array(&salt.to_array());
    env.crypto().sha256(&data).to_bytes()
}

const BOUNTY_REVEAL_DELAY: u32 = 5;

/// One stage of a fully funded, merchant-gated contractor escrow (#209).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MilestoneInput {
    pub title: String,
    pub deliverable_url: String,
    pub amount: i128,
    pub due_timestamp: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MilestoneStage {
    pub title: String,
    pub deliverable_url: String,
    pub amount: i128,
    pub due_timestamp: u64,
    pub proof_url: String,
    pub review_feedback: String,
    pub submitted: bool,
    pub released: bool,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MilestoneProject {
    pub id: u64,
    pub merchant: Address,
    pub contractor: Address,
    pub token: Address,
    pub escrowed: i128,
    pub released_amount: i128,
    pub stages: Vec<MilestoneStage>,
}

/// Storage key for a metered (usage-based) vault (#46). An enum on purpose:
/// `#[contracttype]` structs encode as a map of field names without the type
/// name, so a struct with the same fields as `VaultKey` would share its storage
/// entry. The enum encodes as `["Vault", user, merchant]`, which can't collide.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MeteredVaultKey {
    /// (user, merchant)
    Vault(Address, Address),
}

/// Usage-based allowance (#46): the merchant may pull variable amounts, any
/// number of times, as long as the total within the current epoch stays at or
/// below `max_amount_per_interval`.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MeteredVaultData {
    pub token: Address,
    pub max_amount_per_interval: i128,
    pub interval_seconds: u64,
    /// Ledger timestamp the current billing epoch started at.
    pub current_epoch_start: u64,
    /// Total pulled in the current epoch.
    pub epoch_spent: i128,
}

/// Storage key for the on-chain plan catalog (#95). `plan_id` is unique
/// across the contract so anyone can read a plan back with `get_plan`.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanKey {
    pub plan_id: String,
}

/// Decentralized subscription plan metadata stored on-chain (#95) so
/// third-party dApps can verify plan terms without the off-chain database.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanData {
    pub merchant: Address,
    pub token: Address,
    pub amount_per_interval: i128,
    pub interval_seconds: u64,
    pub name: String,
    pub is_active: bool,
}

// ----------------------------------------------------------------------------
// REVENUE SHARING, RECOVERY, ELASTICITY & GAS SUBSIDY (#127-130)
// ----------------------------------------------------------------------------

/// Public revenue-pool accounting requested by #127. Shareholder membership is
/// immutable after creation so already-accrued entitlements cannot be diluted.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RevenuePool {
    pub total_shares: u32,
    pub shareholders: Vec<(Address, u32)>,
    /// Deposited tokens that have not yet been claimed. This includes any
    /// sub-token-unit dividend dust retained by fixed-point accounting.
    pub undistributed_balance: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RevenuePoolState {
    pub owner: Address,
    pub token: Address,
    pub pool: RevenuePool,
    /// Accumulated token units per share, scaled by `REVENUE_ACC_SCALE`.
    pub acc_per_share: i128,
    /// Scaled numerator remainder carried into the next deposit.
    pub scaled_remainder: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RevenueAccount {
    /// Whole token units already withdrawn by this shareholder.
    pub claimed: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VaultRecoveryConfig {
    pub recovery_address: Address,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VaultRecoveryRequest {
    pub recovery_address: Address,
    pub initiated_at: u64,
    pub execute_after: u64,
    /// Recovery freezes billing immediately. A veto restores this prior state.
    pub was_paused: bool,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ElasticityData {
    pub overdraft_percentage: u32,
    /// Prior-epoch overdraft deducted from this epoch's baseline.
    pub carried_overdraft: i128,
    /// Overdraft consumed above the adjusted baseline in the current epoch.
    pub epoch_overdraft: i128,
}

/// Merchant-funded reimbursement reserve for a configured transaction sponsor.
/// `native_token` must be the network's canonical native-XLM SAC address.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GasSubsidyPool {
    pub native_token: Address,
    pub sponsor: Address,
    pub balance: i128,
    pub max_subsidy_per_handshake: i128,
    pub next_nonce: u64,
    pub locked: bool,
}

/// Namespaced keys prevent collisions with legacy struct-encoded storage keys.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FeatureKey {
    RevenuePool(u64),
    RevenueAccount(u64, Address),
    RecoveryConfig(Address, Address),
    RecoveryRequest(Address, Address),
    Elasticity(Address, Address),
    GasSubsidy(Address),
    YieldStrategy,
}

// ----------------------------------------------------------------------------
// YIELD-BEARING COLLATERAL STRATEGY (#176)
// ----------------------------------------------------------------------------

/// Configuration for the yield strategy adapter. Stores the approved lending
/// pool where idle escrow balances are deployed to generate passive yield.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct YieldStrategyConfig {
    pub yield_pool: Option<Address>,
    pub token: Address,
    pub liquidity_buffer: i128,
    pub total_deployed: i128,
}

/// A trusted Soroban lending market (e.g., Blend Protocol) that accepts token
/// deposits, accrues interest, and allows atomic redemption of principal + yield.
#[contractclient(name = "LendingPoolClient")]
pub trait LendingPool {
    fn supply(env: Env, token: Address, amount: i128, from: Address);
    fn withdraw(env: Env, token: Address, amount: i128, to: Address) -> i128;
    fn balance_of(env: Env, token: Address, account: Address) -> i128;
}

// ----------------------------------------------------------------------------
// SUBSCRIPTION CREDENTIALS, BITMAP INDEX, PAYMENT ROUTING & ESCROW (#123–126)
// ----------------------------------------------------------------------------

pub const DEFAULT_ESCROW_WINDOW_SECONDS: u64 = 86_400;

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SubscriptionKey {
    Badge(Address, Address),
    NextId(Address),
    Bitmap(Address, u32),
    Swap(Address, Address),
    EscrowPolicy(Address, Address),
    Escrow(u64),
    NextEscrowId,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BadgeData {
    pub active: bool,
    pub tier: u32,
    pub expiration_timestamp: u64,
    pub subscriber_id: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SwapConfig {
    pub payment_token: Address,
    pub settlement_token: Address,
    pub router: Address,
    pub max_input_amount: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EscrowPolicy {
    pub arbitrator: Address,
    pub window_seconds: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EscrowHold {
    pub user: Address,
    pub merchant: Address,
    pub token: Address,
    pub amount: i128,
    pub release_at: u64,
    pub arbitrator: Address,
    pub disputed: bool,
}

/// A trusted Soroban AMM router receives only the exact reserved input transferred
/// for this swap (never an approval to drain pooled escrow holdings), sends
/// settlement tokens to `recipient`, refunds unused input, and reports spent input.
/// It must execute atomically and enforce its own route/oracle protections.
#[contractclient(name = "SwapRouterClient")]
pub trait SwapRouter {
    fn swap_exact_output(
        env: Env,
        payment_token: Address,
        settlement_token: Address,
        amount_out: i128,
        max_amount_in: i128,
        recipient: Address,
    ) -> i128;
}

fn extend_key(env: &Env, key: &SubscriptionKey, interval: u64) {
    let (threshold, extend_to) = calculate_ttl_params(interval);
    env.storage()
        .persistent()
        .extend_ttl(key, threshold, extend_to);
}

fn bitmap_status(env: &Env, merchant: &Address, id: u32, active: bool, interval: u64) {
    let key = SubscriptionKey::Bitmap(merchant.clone(), id / 256);
    let mut bytes = env
        .storage()
        .persistent()
        .get::<_, BytesN<32>>(&key)
        .unwrap_or_else(|| BytesN::from_array(env, &[0u8; 32]))
        .to_array();
    let offset = ((id % 256) / 8) as usize;
    let mask = 1u8 << (id % 8);
    if active {
        bytes[offset] |= mask;
    } else {
        bytes[offset] &= !mask;
    }
    env.storage()
        .persistent()
        .set(&key, &BytesN::from_array(env, &bytes));
    extend_key(env, &key, interval);
}

fn activate_badge(
    env: &Env,
    user: &Address,
    merchant: &Address,
    tier: u32,
    until: u64,
    interval: u64,
) {
    let key = SubscriptionKey::Badge(user.clone(), merchant.clone());
    let id = if let Some(existing) = env.storage().persistent().get::<_, BadgeData>(&key) {
        existing.subscriber_id
    } else {
        let next_key = SubscriptionKey::NextId(merchant.clone());
        let next: u32 = env.storage().persistent().get(&next_key).unwrap_or(0);
        env.storage().persistent().set(
            &next_key,
            &next.checked_add(1).expect("Subscriber ID exhausted"),
        );
        extend_key(env, &next_key, interval);
        next
    };
    env.storage().persistent().set(
        &key,
        &BadgeData {
            active: true,
            tier,
            expiration_timestamp: until,
            subscriber_id: id,
        },
    );
    extend_key(env, &key, interval);
    bitmap_status(env, merchant, id, true, interval);
}

fn deactivate_badge(env: &Env, user: &Address, merchant: &Address, interval: u64) {
    let key = SubscriptionKey::Badge(user.clone(), merchant.clone());
    if let Some(mut badge) = env.storage().persistent().get::<_, BadgeData>(&key) {
        badge.active = false;
        env.storage().persistent().set(&key, &badge);
        extend_key(env, &key, interval);
        bitmap_status(env, merchant, badge.subscriber_id, false, interval);
    }
}

/// Performs exact-output conversion into the vault settlement token.
/// Input is collected from the subscriber before invoking the approved router;
/// failure to receive the full output reverts the whole Soroban invocation.
fn pull_payment(
    env: &Env,
    user: &Address,
    merchant: &Address,
    vault: &VaultData,
    recipient: &Address,
) {
    let config_key = SubscriptionKey::Swap(user.clone(), merchant.clone());
    if let Some(config) = env.storage().persistent().get::<_, SwapConfig>(&config_key) {
        assert!(
            config.settlement_token == vault.token,
            "Wrong settlement token"
        );
        assert!(
            config.payment_token != config.settlement_token,
            "Swap requires distinct tokens"
        );
        assert!(config.max_input_amount > 0, "Invalid maximum input");
        let contract = env.current_contract_address();
        let input = token::Client::new(env, &config.payment_token);
        let output = token::Client::new(env, &config.settlement_token);
        let input_before = input.balance(&contract);
        let output_before = output.balance(&contract);
        input.transfer_from(&contract, user, &config.router, &config.max_input_amount);
        let spent = SwapRouterClient::new(env, &config.router).swap_exact_output(
            &config.payment_token,
            &config.settlement_token,
            &vault.amount_per_interval,
            &config.max_input_amount,
            &contract,
        );
        assert!(
            spent > 0 && spent <= config.max_input_amount,
            "Slippage limit exceeded"
        );
        let received = output.balance(&contract) - output_before;
        assert!(
            received >= vault.amount_per_interval,
            "Insufficient settlement output"
        );
        let unspent = input.balance(&contract) - input_before;
        assert!(
            unspent >= 0 && unspent <= config.max_input_amount,
            "Invalid swap input refund"
        );
        assert!(
            config.max_input_amount - unspent == spent,
            "Router input accounting mismatch"
        );
        if unspent > 0 {
            input.transfer(&contract, user, &unspent);
        }
        if received > vault.amount_per_interval {
            output.transfer(&contract, user, &(received - vault.amount_per_interval));
        }
        // Escrow settlement is already held by this contract; avoid a
        // redundant self-transfer and keep the received balance unchanged.
        if recipient != &contract {
            output.transfer(&contract, recipient, &vault.amount_per_interval);
        }
        extend_key(env, &config_key, vault.interval_seconds);
    } else {
        token::Client::new(env, &vault.token).transfer_from(
            &env.current_contract_address(),
            user,
            recipient,
            &vault.amount_per_interval,
        );
    }
}

fn store_escrow(
    env: &Env,
    user: &Address,
    merchant: &Address,
    vault: &VaultData,
    policy: &EscrowPolicy,
) -> u64 {
    let counter_key = SubscriptionKey::NextEscrowId;
    let next: u64 = env.storage().instance().get(&counter_key).unwrap_or(0);
    env.storage().instance().set(
        &counter_key,
        &next.checked_add(1).expect("Escrow ID exhausted"),
    );
    let hold_key = SubscriptionKey::Escrow(next);
    let hold = EscrowHold {
        user: user.clone(),
        merchant: merchant.clone(),
        token: vault.token.clone(),
        amount: vault.amount_per_interval,
        release_at: env
            .ledger()
            .timestamp()
            .saturating_add(policy.window_seconds),
        arbitrator: policy.arbitrator.clone(),
        disputed: false,
    };
    env.storage().persistent().set(&hold_key, &hold);
    extend_key(env, &hold_key, policy.window_seconds);
    next
}

// ----------------------------------------------------------------------------
// STORAGE LAYOUT
// ----------------------------------------------------------------------------
//
// Vaults live in persistent storage for the lifetime of the subscription. The
// key is the `VaultKey` struct and the value is the full `VaultData` record.

fn vault_key(user: &Address, merchant: &Address) -> VaultKey {
    VaultKey {
        user: user.clone(),
        merchant: merchant.clone(),
    }
}

fn load_vault(env: &Env, key: &VaultKey) -> Option<VaultData> {
    env.storage().persistent().get(key)
}

fn save_vault(env: &Env, key: &VaultKey, vault: &VaultData) {
    env.storage().persistent().set(key, vault);
}

fn extend_feature_key(env: &Env, key: &FeatureKey, lifetime_seconds: u64) {
    let (threshold, extend_to) = calculate_ttl_params(lifetime_seconds);
    env.storage()
        .persistent()
        .extend_ttl(key, threshold, extend_to);
}

fn shareholder_weight(state: &RevenuePoolState, shareholder: &Address) -> u32 {
    for entry in state.pool.shareholders.iter() {
        if entry.0 == *shareholder {
            return entry.1;
        }
    }
    panic!("Address is not a shareholder")
}

fn clear_recovery_state(env: &Env, user: &Address, merchant: &Address) {
    env.storage()
        .persistent()
        .remove(&FeatureKey::RecoveryConfig(user.clone(), merchant.clone()));
    env.storage()
        .persistent()
        .remove(&FeatureKey::RecoveryRequest(user.clone(), merchant.clone()));
}

// ----------------------------------------------------------------------------
// CONTRACT LOGIC
// ----------------------------------------------------------------------------

/// Validates the terms and writes a fresh vault for (user, merchant), emitting
/// `vault_created` (or `trial_vault_created`) and extending storage TTLs.
/// Shared by the handshake entrypoints so every path behaves identically.
fn write_vault(
    env: &Env,
    user: &Address,
    merchant: &Address,
    token: &Address,
    amount_per_interval: i128,
    interval_seconds: u64,
    grace_period_seconds: u64,
    trial_end_timestamp: u64,
) {
    assert!(
        amount_per_interval > 0,
        "amount_per_interval must be positive"
    );
    assert!(
        interval_seconds > 0,
        "interval_seconds must be greater than zero"
    );

    let key = VaultKey {
        user: user.clone(),
        merchant: merchant.clone(),
    };

    let vault_data = VaultData {
        token: token.clone(),
        amount_per_interval,
        interval_seconds,
        last_pull_timestamp: 0,
        grace_period_seconds,
        payment_status: PAYMENT_ACTIVE,
        initial_timestamp: env.ledger().timestamp(),
        pull_sequence: PULL_SEQUENCE_UNSET,
        is_paused: false,
        trial_end_timestamp,
    };

    env.storage().persistent().set(&key, &vault_data);

    // A replacement subscription resets payment policies, but retains the
    // sequential badge identity previously assigned to this subscriber.
    env.storage()
        .persistent()
        .remove(&SubscriptionKey::Swap(user.clone(), merchant.clone()));
    env.storage()
        .persistent()
        .remove(&SubscriptionKey::EscrowPolicy(
            user.clone(),
            merchant.clone(),
        ));
    clear_recovery_state(env, user, merchant);
    let tier = env
        .storage()
        .persistent()
        .get::<_, BadgeData>(&SubscriptionKey::Badge(user.clone(), merchant.clone()))
        .map(|badge| badge.tier)
        .unwrap_or(1);
    activate_badge(
        env,
        user,
        merchant,
        tier,
        env.ledger().timestamp().saturating_add(interval_seconds),
        interval_seconds,
    );

    let event = if trial_end_timestamp > 0 {
        Symbol::new(env, "trial_vault_created")
    } else {
        Symbol::new(env, "vault_created")
    };
    env.events().publish(
        (event, user.clone(), merchant.clone()),
        (
            token.clone(),
            amount_per_interval,
            interval_seconds,
            trial_end_timestamp,
        ),
    );
    // Extend vault storage TTL and contract instance TTL to prevent archival
    let (threshold, extend_to) = calculate_ttl_params(interval_seconds);
    env.storage()
        .persistent()
        .extend_ttl(&key, threshold, extend_to);
    env.storage().instance().extend_ttl(threshold, extend_to);
}

#[contract]
pub struct OrbitContract;

#[contractimpl]
impl OrbitContract {
    // --------------------------------------------------------------------
    // PROPORTIONAL REVENUE POOLS (#127)
    // --------------------------------------------------------------------

    /// Create an immutable-weight revenue pool. Immutability avoids retroactive
    /// dilution and keeps cumulative entitlement math auditable.
    pub fn create_revenue_pool(
        env: Env,
        owner: Address,
        pool_id: u64,
        token: Address,
        shareholders: Vec<(Address, u32)>,
    ) {
        owner.require_auth();
        assert!(!shareholders.is_empty(), "Shareholders must not be empty");
        assert!(
            shareholders.len() <= MAX_REVENUE_POOL_SHAREHOLDERS,
            "Too many shareholders"
        );

        let key = FeatureKey::RevenuePool(pool_id);
        assert!(
            !env.storage().persistent().has(&key),
            "Revenue pool already exists"
        );

        let mut total_shares = 0u32;
        for (index, entry) in shareholders.iter().enumerate() {
            assert!(entry.1 > 0, "Share weight must be positive");
            for previous_index in 0..index as u32 {
                let previous = shareholders
                    .get(previous_index)
                    .expect("Shareholder index missing");
                assert!(previous.0 != entry.0, "Duplicate shareholder");
            }
            total_shares = total_shares
                .checked_add(entry.1)
                .expect("Total shares overflow");
        }

        let state = RevenuePoolState {
            owner: owner.clone(),
            token: token.clone(),
            pool: RevenuePool {
                total_shares,
                shareholders,
                undistributed_balance: 0,
            },
            acc_per_share: 0,
            scaled_remainder: 0,
        };
        env.storage().persistent().set(&key, &state);
        extend_feature_key(&env, &key, MIN_TTL_LEDGERS as u64 * SECONDS_PER_LEDGER);
        env.events().publish(
            (Symbol::new(&env, "revenue_pool_created"), pool_id),
            (owner, token, total_shares),
        );
    }

    /// Deposit revenue and add it to the cumulative per-share accumulator. The
    /// division remainder is carried into later deposits, rather than discarded.
    pub fn deposit_to_pool(env: Env, sender: Address, pool_id: u64, token: Address, amount: i128) {
        sender.require_auth();
        assert!(amount > 0, "Deposit amount must be positive");

        let key = FeatureKey::RevenuePool(pool_id);
        let mut state: RevenuePoolState = env
            .storage()
            .persistent()
            .get(&key)
            .expect("Revenue pool does not exist");
        assert!(state.token == token, "Wrong revenue pool token");

        let scaled = amount
            .checked_mul(REVENUE_ACC_SCALE)
            .expect("Revenue accumulator overflow")
            .checked_add(state.scaled_remainder)
            .expect("Revenue accumulator overflow");
        let total_shares = i128::from(state.pool.total_shares);
        let increment = scaled / total_shares;
        state.scaled_remainder = scaled % total_shares;
        state.acc_per_share = state
            .acc_per_share
            .checked_add(increment)
            .expect("Revenue accumulator overflow");
        state.pool.undistributed_balance = state
            .pool
            .undistributed_balance
            .checked_add(amount)
            .expect("Revenue balance overflow");

        // Effects precede the external token call; a failed transfer reverts all.
        env.storage().persistent().set(&key, &state);
        token::Client::new(&env, &token).transfer_from(
            &env.current_contract_address(),
            &sender,
            &env.current_contract_address(),
            &amount,
        );
        extend_feature_key(&env, &key, MIN_TTL_LEDGERS as u64 * SECONDS_PER_LEDGER);
        env.events().publish(
            (Symbol::new(&env, "revenue_deposited"), pool_id),
            (sender, token, amount, state.acc_per_share),
        );
    }

    /// Claim all whole-token units accrued to `shareholder` and not previously
    /// withdrawn. Fractional fixed-point entitlement remains for later deposits.
    pub fn claim_shareholder_dividends(env: Env, shareholder: Address, pool_id: u64) -> i128 {
        shareholder.require_auth();
        let pool_key = FeatureKey::RevenuePool(pool_id);
        let mut state: RevenuePoolState = env
            .storage()
            .persistent()
            .get(&pool_key)
            .expect("Revenue pool does not exist");
        let weight = i128::from(shareholder_weight(&state, &shareholder));
        let gross_entitlement = weight
            .checked_mul(state.acc_per_share)
            .expect("Dividend entitlement overflow")
            / REVENUE_ACC_SCALE;

        let account_key = FeatureKey::RevenueAccount(pool_id, shareholder.clone());
        let mut account: RevenueAccount = env
            .storage()
            .persistent()
            .get(&account_key)
            .unwrap_or(RevenueAccount { claimed: 0 });
        let amount = gross_entitlement
            .checked_sub(account.claimed)
            .expect("Invalid dividend account");
        assert!(amount > 0, "No dividends available");
        assert!(
            amount <= state.pool.undistributed_balance,
            "Insufficient pool balance"
        );

        account.claimed = gross_entitlement;
        state.pool.undistributed_balance -= amount;
        env.storage().persistent().set(&account_key, &account);
        env.storage().persistent().set(&pool_key, &state);
        token::Client::new(&env, &state.token).transfer(
            &env.current_contract_address(),
            &shareholder,
            &amount,
        );
        extend_feature_key(&env, &pool_key, MIN_TTL_LEDGERS as u64 * SECONDS_PER_LEDGER);
        extend_feature_key(
            &env,
            &account_key,
            MIN_TTL_LEDGERS as u64 * SECONDS_PER_LEDGER,
        );
        env.events().publish(
            (Symbol::new(&env, "revenue_claimed"), pool_id),
            (shareholder, amount),
        );
        amount
    }

    pub fn get_revenue_pool(env: Env, pool_id: u64) -> Option<RevenuePoolState> {
        env.storage()
            .persistent()
            .get(&FeatureKey::RevenuePool(pool_id))
    }

    // --------------------------------------------------------------------
    // MERCHANT-FUNDED SPONSOR REIMBURSEMENT (#130)
    // --------------------------------------------------------------------

    /// Configure a reimbursement reserve. The merchant must provide the
    /// canonical native-XLM SAC address for the deployment network and a
    /// trusted off-chain transaction sponsor.
    pub fn configure_gas_subsidy(
        env: Env,
        merchant: Address,
        native_token: Address,
        sponsor: Address,
        max_subsidy_per_handshake: i128,
    ) {
        merchant.require_auth();
        assert!(
            max_subsidy_per_handshake > 0,
            "Maximum subsidy must be positive"
        );
        let key = FeatureKey::GasSubsidy(merchant.clone());
        let pool = if let Some(existing) = env.storage().persistent().get::<_, GasSubsidyPool>(&key)
        {
            assert!(!existing.locked, "Gas subsidy pool is locked");
            assert!(
                existing.native_token == native_token,
                "Cannot change funded subsidy token"
            );
            assert!(
                existing.sponsor == sponsor,
                "Cannot change configured sponsor"
            );
            GasSubsidyPool {
                max_subsidy_per_handshake,
                ..existing
            }
        } else {
            GasSubsidyPool {
                native_token,
                sponsor,
                balance: 0,
                max_subsidy_per_handshake,
                next_nonce: 0,
                locked: false,
            }
        };
        env.storage().persistent().set(&key, &pool);
        extend_feature_key(&env, &key, MIN_TTL_LEDGERS as u64 * SECONDS_PER_LEDGER);
    }

    pub fn deposit_gas_subsidy(env: Env, merchant: Address, amount: i128) {
        merchant.require_auth();
        assert!(amount > 0, "Subsidy deposit must be positive");
        let key = FeatureKey::GasSubsidy(merchant.clone());
        let mut pool: GasSubsidyPool = env
            .storage()
            .persistent()
            .get(&key)
            .expect("Gas subsidy pool is not configured");
        assert!(!pool.locked, "Gas subsidy pool is locked");
        pool.balance = pool
            .balance
            .checked_add(amount)
            .expect("Gas subsidy balance overflow");
        pool.locked = true;
        env.storage().persistent().set(&key, &pool);
        token::Client::new(&env, &pool.native_token).transfer_from(
            &env.current_contract_address(),
            &merchant,
            &env.current_contract_address(),
            &amount,
        );
        pool.locked = false;
        env.storage().persistent().set(&key, &pool);
        extend_feature_key(&env, &key, MIN_TTL_LEDGERS as u64 * SECONDS_PER_LEDGER);
        env.events().publish(
            (Symbol::new(&env, "gas_subsidy_deposited"), merchant),
            (amount, pool.balance),
        );
    }

    pub fn withdraw_gas_subsidy(env: Env, merchant: Address, amount: i128) {
        merchant.require_auth();
        assert!(amount > 0, "Subsidy withdrawal must be positive");
        let key = FeatureKey::GasSubsidy(merchant.clone());
        let mut pool: GasSubsidyPool = env
            .storage()
            .persistent()
            .get(&key)
            .expect("Gas subsidy pool is not configured");
        assert!(!pool.locked, "Gas subsidy pool is locked");
        assert!(amount <= pool.balance, "Insufficient subsidy balance");
        pool.balance -= amount;
        pool.locked = true;
        env.storage().persistent().set(&key, &pool);
        token::Client::new(&env, &pool.native_token).transfer(
            &env.current_contract_address(),
            &merchant,
            &amount,
        );
        pool.locked = false;
        env.storage().persistent().set(&key, &pool);
        env.events().publish(
            (Symbol::new(&env, "gas_subsidy_withdrawn"), merchant),
            (amount, pool.balance),
        );
    }

    /// Create a vault while reimbursing a configured transaction sponsor from
    /// the merchant reserve. The sponsor must still submit/pay the transaction;
    /// Soroban contracts cannot pay their own invocation fee.
    pub fn create_sponsored_vault(
        env: Env,
        user: Address,
        merchant: Address,
        token: Address,
        amount_per_interval: i128,
        interval_seconds: u64,
        subsidy_amount: i128,
        nonce: u64,
    ) {
        user.require_auth();
        assert!(subsidy_amount > 0, "Subsidy amount must be positive");
        let subsidy_key = FeatureKey::GasSubsidy(merchant.clone());
        let mut pool: GasSubsidyPool = env
            .storage()
            .persistent()
            .get(&subsidy_key)
            .expect("Gas subsidy pool is not configured");
        pool.sponsor.require_auth();
        assert!(!pool.locked, "Gas subsidy pool is locked");
        assert!(
            subsidy_amount <= pool.max_subsidy_per_handshake,
            "Subsidy exceeds per-handshake cap"
        );
        assert!(
            subsidy_amount <= pool.balance,
            "Insufficient subsidy balance"
        );
        assert!(nonce == pool.next_nonce, "Invalid subsidy nonce");

        pool.balance -= subsidy_amount;
        pool.next_nonce = pool
            .next_nonce
            .checked_add(1)
            .expect("Subsidy nonce exhausted");
        pool.locked = true;
        env.storage().persistent().set(&subsidy_key, &pool);

        write_vault(
            &env,
            &user,
            &merchant,
            &token,
            amount_per_interval,
            interval_seconds,
            DEFAULT_GRACE_PERIOD_SECONDS,
            0,
        );
        token::Client::new(&env, &pool.native_token).transfer(
            &env.current_contract_address(),
            &pool.sponsor,
            &subsidy_amount,
        );

        pool.locked = false;
        env.storage().persistent().set(&subsidy_key, &pool);
        env.events().publish(
            (Symbol::new(&env, "vault_sponsored"), user, merchant),
            (pool.sponsor, subsidy_amount, nonce),
        );
    }

    pub fn get_gas_subsidy_pool(env: Env, merchant: Address) -> Option<GasSubsidyPool> {
        env.storage()
            .persistent()
            .get(&FeatureKey::GasSubsidy(merchant))
    }

    /// 1. THE HANDSHAKE (Create Allowance)
    /// The user calls this to set up a recurring subscription for a merchant.
    pub fn create_vault(
        env: Env,
        user: Address,
        merchant: Address,
        token: Address,
        amount_per_interval: i128,
        interval_seconds: u64,
    ) {
        Self::create_vault_with_grace_period(
            env,
            user,
            merchant,
            token,
            amount_per_interval,
            interval_seconds,
            DEFAULT_GRACE_PERIOD_SECONDS,
        );
    }

    pub fn create_vault_with_grace_period(
        env: Env,
        user: Address,
        merchant: Address,
        token: Address,
        amount_per_interval: i128,
        interval_seconds: u64,
        grace_period_seconds: u64,
    ) {
        user.require_auth();
        assert!(
            grace_period_seconds > 0,
            "grace_period_seconds must be greater than zero"
        );

        write_vault(
            &env,
            &user,
            &merchant,
            &token,
            amount_per_interval,
            interval_seconds,
            grace_period_seconds,
            0,
        );
    }

    /// Create a subscription that starts with a free-trial window. Merchant
    /// pulls are blocked until `trial_end_timestamp`, and the subscriber can
    /// cancel at any point during the trial without funds being deducted.
    pub fn create_vault_with_trial(
        env: Env,
        user: Address,
        merchant: Address,
        token: Address,
        amount_per_interval: i128,
        interval_seconds: u64,
        trial_duration_seconds: u64,
    ) {
        user.require_auth();
        assert!(
            trial_duration_seconds > 0,
            "trial_duration_seconds must be greater than zero"
        );

        let trial_end_timestamp = env
            .ledger()
            .timestamp()
            .checked_add(trial_duration_seconds)
            .expect("Trial end overflow");

        write_vault(
            &env,
            &user,
            &merchant,
            &token,
            amount_per_interval,
            interval_seconds,
            DEFAULT_GRACE_PERIOD_SECONDS,
            trial_end_timestamp,
        );
    }

    /// Create a fixed-interval vault with a separately designated recovery
    /// address. Existing creation entrypoints remain recovery-disabled.
    pub fn create_vault_with_recovery(
        env: Env,
        user: Address,
        merchant: Address,
        token: Address,
        amount_per_interval: i128,
        interval_seconds: u64,
        recovery_address: Address,
    ) {
        user.require_auth();
        assert!(
            recovery_address != user,
            "Recovery address must differ from user"
        );
        write_vault(
            &env,
            &user,
            &merchant,
            &token,
            amount_per_interval,
            interval_seconds,
            DEFAULT_GRACE_PERIOD_SECONDS,
            0,
        );

        let recovery_key = FeatureKey::RecoveryConfig(user.clone(), merchant.clone());
        env.storage().persistent().set(
            &recovery_key,
            &VaultRecoveryConfig {
                recovery_address: recovery_address.clone(),
            },
        );
        extend_feature_key(&env, &recovery_key, interval_seconds);
        env.events().publish(
            (Symbol::new(&env, "recovery_configured"), user, merchant),
            recovery_address,
        );
    }

    pub fn get_vault_recovery(
        env: Env,
        user: Address,
        merchant: Address,
    ) -> Option<VaultRecoveryRequest> {
        env.storage()
            .persistent()
            .get(&FeatureKey::RecoveryRequest(user, merchant))
    }

    /// The designated address starts the challenge window and immediately
    /// freezes merchant pulls so a lost-key vault cannot keep draining.
    pub fn initiate_vault_recovery(
        env: Env,
        recovery_address: Address,
        user: Address,
        merchant: Address,
    ) {
        recovery_address.require_auth();
        let config_key = FeatureKey::RecoveryConfig(user.clone(), merchant.clone());
        let config: VaultRecoveryConfig = env
            .storage()
            .persistent()
            .get(&config_key)
            .expect("Vault recovery is not configured");
        assert!(
            config.recovery_address == recovery_address,
            "Not the designated recovery address"
        );
        let request_key = FeatureKey::RecoveryRequest(user.clone(), merchant.clone());
        assert!(
            !env.storage().persistent().has(&request_key),
            "Recovery already pending"
        );

        let vault_key = vault_key(&user, &merchant);
        let mut vault = load_vault(&env, &vault_key).expect("Vault does not exist");
        let now = env.ledger().timestamp();
        let execute_after = now
            .checked_add(RECOVERY_TIMELOCK_SECONDS)
            .expect("Recovery deadline overflow");
        let request = VaultRecoveryRequest {
            recovery_address: recovery_address.clone(),
            initiated_at: now,
            execute_after,
            was_paused: vault.is_paused,
        };
        vault.is_paused = true;
        save_vault(&env, &vault_key, &vault);
        deactivate_badge(&env, &user, &merchant, vault.interval_seconds);
        env.storage().persistent().set(&request_key, &request);
        extend_feature_key(&env, &config_key, RECOVERY_TIMELOCK_SECONDS);
        extend_feature_key(&env, &request_key, RECOVERY_TIMELOCK_SECONDS);
        env.events().publish(
            (Symbol::new(&env, "recovery_initiated"), user, merchant),
            (recovery_address, execute_after),
        );
    }

    /// The original key can veto a pending request at any point before it is
    /// executed and restores the vault's prior pause state.
    pub fn cancel_recovery(env: Env, user: Address, merchant: Address) {
        user.require_auth();
        let request_key = FeatureKey::RecoveryRequest(user.clone(), merchant.clone());
        let request: VaultRecoveryRequest = env
            .storage()
            .persistent()
            .get(&request_key)
            .expect("Recovery request does not exist");
        let vault_key = vault_key(&user, &merchant);
        let mut vault = load_vault(&env, &vault_key).expect("Vault does not exist");
        vault.is_paused = request.was_paused;
        save_vault(&env, &vault_key, &vault);
        if !vault.is_paused {
            let tier = env
                .storage()
                .persistent()
                .get::<_, BadgeData>(&SubscriptionKey::Badge(user.clone(), merchant.clone()))
                .map(|badge| badge.tier)
                .unwrap_or(1);
            activate_badge(
                &env,
                &user,
                &merchant,
                tier,
                env.ledger()
                    .timestamp()
                    .saturating_add(vault.interval_seconds),
                vault.interval_seconds,
            );
        }
        env.storage().persistent().remove(&request_key);
        env.events().publish(
            (Symbol::new(&env, "recovery_cancelled"), user, merchant),
            (),
        );
    }

    /// After the challenge window, replace the old vault with a paused vault
    /// owned by the recovery address. The recovered account must approve the
    /// token allowance and call `resume_vault` before billing can restart.
    pub fn execute_vault_recovery(
        env: Env,
        recovery_address: Address,
        user: Address,
        merchant: Address,
    ) {
        recovery_address.require_auth();
        let request_key = FeatureKey::RecoveryRequest(user.clone(), merchant.clone());
        let request: VaultRecoveryRequest = env
            .storage()
            .persistent()
            .get(&request_key)
            .expect("Recovery request does not exist");
        assert!(
            request.recovery_address == recovery_address,
            "Not the designated recovery address"
        );
        assert!(
            env.ledger().timestamp() >= request.execute_after,
            "Recovery timelock has not elapsed"
        );

        let old_key = vault_key(&user, &merchant);
        let old_vault = load_vault(&env, &old_key).expect("Vault does not exist");
        let new_key = vault_key(&recovery_address, &merchant);
        assert!(
            !env.storage().persistent().has(&new_key),
            "Recovery destination vault already exists"
        );

        let now = env.ledger().timestamp();
        let replacement = VaultData {
            token: old_vault.token.clone(),
            amount_per_interval: old_vault.amount_per_interval,
            interval_seconds: old_vault.interval_seconds,
            last_pull_timestamp: now,
            grace_period_seconds: old_vault.grace_period_seconds,
            payment_status: PAYMENT_ACTIVE,
            initial_timestamp: now,
            pull_sequence: PULL_SEQUENCE_UNSET,
            is_paused: true,
            trial_end_timestamp: 0,
        };

        // Remove the old contract-level connection before creating the new one.
        deactivate_badge(&env, &user, &merchant, old_vault.interval_seconds);
        env.storage().persistent().remove(&old_key);
        env.storage()
            .persistent()
            .remove(&SubscriptionKey::Swap(user.clone(), merchant.clone()));
        env.storage()
            .persistent()
            .remove(&SubscriptionKey::EscrowPolicy(
                user.clone(),
                merchant.clone(),
            ));
        clear_recovery_state(&env, &user, &merchant);

        save_vault(&env, &new_key, &replacement);
        activate_badge(
            &env,
            &recovery_address,
            &merchant,
            1,
            now.saturating_add(old_vault.interval_seconds),
            old_vault.interval_seconds,
        );
        deactivate_badge(
            &env,
            &recovery_address,
            &merchant,
            old_vault.interval_seconds,
        );
        let (threshold, extend_to) = calculate_ttl_params(old_vault.interval_seconds);
        env.storage()
            .persistent()
            .extend_ttl(&new_key, threshold, extend_to);
        env.events().publish(
            (Symbol::new(&env, "vault_recovered"), user, merchant),
            recovery_address,
        );
    }

    /// Contract-native soulbound credential: no transfer or approval method
    /// exists; third-party contracts can call this public read once per pair.
    pub fn get_subscriber_badge(env: Env, user: Address, merchant: Address) -> Option<BadgeData> {
        env.storage()
            .persistent()
            .get(&SubscriptionKey::Badge(user, merchant))
    }

    pub fn is_subscriber_active(env: Env, user: Address, merchant: Address) -> bool {
        let key = VaultKey {
            user: user.clone(),
            merchant: merchant.clone(),
        };
        let vault: Option<VaultData> = env.storage().persistent().get(&key);
        let badge: Option<BadgeData> = env
            .storage()
            .persistent()
            .get(&SubscriptionKey::Badge(user, merchant));
        match (vault, badge) {
            (Some(vault), Some(badge)) => {
                badge.active
                    && !vault.is_paused
                    && env.ledger().timestamp() < badge.expiration_timestamp
            }
            _ => false,
        }
    }

    /// Merchant-assigned tier, without allowing arbitrary minting by a user.
    pub fn set_subscriber_tier(env: Env, user: Address, merchant: Address, tier: u32) {
        merchant.require_auth();
        assert!(tier > 0, "Tier must be positive");
        let key = SubscriptionKey::Badge(user.clone(), merchant.clone());
        let mut badge: BadgeData = env
            .storage()
            .persistent()
            .get(&key)
            .expect("Subscription badge does not exist");
        assert!(
            env.storage().persistent().has(&VaultKey { user, merchant }),
            "Vault does not exist"
        );
        badge.tier = tier;
        env.storage().persistent().set(&key, &badge);
    }

    /// Permissionless keeper hook synchronizes a time-expired credential with
    /// its packed bitmap. Validity reads also enforce expiration independently.
    pub fn mark_subscription_default(env: Env, user: Address, merchant: Address) -> bool {
        let key = SubscriptionKey::Badge(user.clone(), merchant.clone());
        if let Some(badge) = env.storage().persistent().get::<_, BadgeData>(&key) {
            if badge.active && env.ledger().timestamp() >= badge.expiration_timestamp {
                let interval = env
                    .storage()
                    .persistent()
                    .get::<_, VaultData>(&VaultKey {
                        user: user.clone(),
                        merchant: merchant.clone(),
                    })
                    .map(|v| v.interval_seconds)
                    .unwrap_or(DEFAULT_ESCROW_WINDOW_SECONDS);
                deactivate_badge(&env, &user, &merchant, interval);
                return true;
            }
        }
        false
    }

    pub fn get_subscriber_bitmap_chunk(
        env: Env,
        merchant: Address,
        chunk_index: u32,
    ) -> BytesN<32> {
        env.storage()
            .persistent()
            .get(&SubscriptionKey::Bitmap(merchant, chunk_index))
            .unwrap_or_else(|| BytesN::from_array(&env, &[0u8; 32]))
    }

    /// Configure a per-vault cross-asset exact-output Soroban AMM router.
    /// Both parties consent to the trusted router, token choice, and cap.
    pub fn configure_swap(
        env: Env,
        user: Address,
        merchant: Address,
        payment_token: Address,
        router: Address,
        max_input_amount: i128,
    ) {
        user.require_auth();
        merchant.require_auth();
        assert!(max_input_amount > 0, "Maximum input must be positive");
        let vault: VaultData = env
            .storage()
            .persistent()
            .get(&VaultKey {
                user: user.clone(),
                merchant: merchant.clone(),
            })
            .expect("Vault does not exist");
        assert!(
            payment_token != vault.token,
            "Use regular billing for same-token settlement"
        );
        let key = SubscriptionKey::Swap(user, merchant);
        env.storage().persistent().set(
            &key,
            &SwapConfig {
                payment_token,
                settlement_token: vault.token,
                router,
                max_input_amount,
            },
        );
        extend_key(&env, &key, vault.interval_seconds);
    }

    pub fn clear_swap(env: Env, user: Address, merchant: Address) {
        user.require_auth();
        env.storage()
            .persistent()
            .remove(&SubscriptionKey::Swap(user, merchant));
    }

    /// Merchant pre-approves a Soroban AMM router that subscribers may use to pay
    /// in another asset (#191). Routers that are not approved here can never back
    /// a swap subscription, so the subscriber can't pick an arbitrary router.
    pub fn approve_swap_router(env: Env, merchant: Address, router: Address) {
        merchant.require_auth();
        let key = DataKey::TrustedSwapRouter(merchant, router);
        env.storage().persistent().set(&key, &true);
        env.storage()
            .persistent()
            .extend_ttl(&key, TTL_THRESHOLD_LEDGERS, MIN_TTL_LEDGERS);
    }

    /// Withdraws a merchant's approval of a swap router. Existing swap vaults that
    /// use the router stop settling through it on their next pull.
    pub fn revoke_swap_router(env: Env, merchant: Address, router: Address) {
        merchant.require_auth();
        env.storage()
            .persistent()
            .remove(&DataKey::TrustedSwapRouter(merchant, router));
    }

    /// Subscribe and set up a cross-asset payment in a single invocation (#191).
    /// The subscriber pays `payment_token`, the merchant settles in
    /// `settlement_token`, and `router` converts between them on each pull.
    ///
    /// Vault creation, the swap config, and the payment-token allowance all happen
    /// here, so one subscriber signature covers them and any failure reverts the
    /// whole call. Soroban allows one contract invocation per transaction, so
    /// this is how the subscribe-and-swap step stays atomic.
    pub fn create_vault_with_swap(
        env: Env,
        user: Address,
        merchant: Address,
        payment_token: Address,
        settlement_token: Address,
        router: Address,
        amount_per_interval: i128,
        interval_seconds: u64,
        max_input_amount: i128,
        payment_allowance: i128,
        allowance_live_until_ledger: u32,
    ) {
        user.require_auth();
        assert!(max_input_amount > 0, "Maximum input must be positive");
        assert!(
            payment_token != settlement_token,
            "Use regular billing for same-token settlement"
        );
        assert!(
            payment_allowance >= max_input_amount,
            "Allowance must cover the maximum input"
        );
        assert!(
            env.storage()
                .persistent()
                .get(&DataKey::TrustedSwapRouter(
                    merchant.clone(),
                    router.clone()
                ))
                .unwrap_or(false),
            "Router is not approved by this merchant"
        );

        // The user already authorized this entrypoint. Calling the public
        // `create_vault` entrypoint here would request the same auth frame a
        // second time and Soroban rejects that duplicate authorization.
        // Write the vault directly: `create_vault` would require `user` auth a
        // second time in this frame, which the host rejects.
        write_vault(
            &env,
            &user,
            &merchant,
            &settlement_token,
            amount_per_interval,
            interval_seconds,
            DEFAULT_GRACE_PERIOD_SECONDS,
            0,
        );

        token::Client::new(&env, &payment_token).approve(
            &user,
            &env.current_contract_address(),
            &payment_allowance,
            &allowance_live_until_ledger,
        );

        let key = SubscriptionKey::Swap(user.clone(), merchant.clone());
        env.storage().persistent().set(
            &key,
            &SwapConfig {
                payment_token: payment_token.clone(),
                settlement_token: settlement_token.clone(),
                router,
                max_input_amount,
            },
        );
        extend_key(&env, &key, interval_seconds);

        env.events().publish(
            (Symbol::new(&env, "vault_swap_configured"), user, merchant),
            (payment_token, settlement_token, max_input_amount),
        );
    }

    /// Opt into dispute-protected settlement with a pre-agreed arbitrator.
    /// Legacy vaults continue immediate settlement for backward compatibility.
    pub fn configure_escrow(
        env: Env,
        user: Address,
        merchant: Address,
        arbitrator: Address,
        escrow_window_seconds: u64,
    ) {
        user.require_auth();
        merchant.require_auth();
        assert!(escrow_window_seconds > 0, "Escrow window must be positive");
        let vault: VaultData = env
            .storage()
            .persistent()
            .get(&VaultKey {
                user: user.clone(),
                merchant: merchant.clone(),
            })
            .expect("Vault does not exist");
        let key = SubscriptionKey::EscrowPolicy(user, merchant);
        env.storage().persistent().set(
            &key,
            &EscrowPolicy {
                arbitrator,
                window_seconds: escrow_window_seconds,
            },
        );
        extend_key(
            &env,
            &key,
            vault.interval_seconds.max(escrow_window_seconds),
        );
    }

    /// Subscriber-only dispute during an open claim window. A disputed hold
    /// cannot be claimed until the designated arbitrator resolves it.
    pub fn raise_dispute(env: Env, user: Address, merchant: Address, pull_id: u64) {
        user.require_auth();
        let key = SubscriptionKey::Escrow(pull_id);
        let mut hold: EscrowHold = env
            .storage()
            .persistent()
            .get(&key)
            .expect("Escrow does not exist");
        assert!(
            hold.user == user && hold.merchant == merchant,
            "Wrong escrow parties"
        );
        assert!(
            env.ledger().timestamp() < hold.release_at,
            "Dispute window closed"
        );
        assert!(!hold.disputed, "Already disputed");
        hold.disputed = true;
        env.storage().persistent().set(&key, &hold);
    }

    /// The escrow's designated arbitrator is the only entity authorized to
    /// apportion funds. Percentages outside 0–100 are rejected.
    pub fn resolve_dispute(env: Env, arbitrator: Address, pull_id: u64, refund_percentage: u32) {
        arbitrator.require_auth();
        assert!(refund_percentage <= 100, "Invalid refund percentage");
        let key = SubscriptionKey::Escrow(pull_id);
        let hold: EscrowHold = env
            .storage()
            .persistent()
            .get(&key)
            .expect("Escrow does not exist");
        assert!(hold.disputed, "Escrow is not disputed");
        assert!(hold.arbitrator == arbitrator, "Unauthorized arbitrator");
        env.storage().persistent().remove(&key);
        let refund = (hold.amount / 100) * i128::from(refund_percentage)
            + ((hold.amount % 100) * i128::from(refund_percentage) / 100);
        let contract = env.current_contract_address();
        let token = token::Client::new(&env, &hold.token);
        if refund > 0 {
            token.transfer(&contract, &hold.user, &refund);
        }
        if hold.amount > refund {
            token.transfer(&contract, &hold.merchant, &(hold.amount - refund));
        }
    }

    pub fn claim_escrow_funds(env: Env, merchant: Address, pull_id: u64) {
        merchant.require_auth();
        let key = SubscriptionKey::Escrow(pull_id);
        let hold: EscrowHold = env
            .storage()
            .persistent()
            .get(&key)
            .expect("Escrow does not exist");
        assert!(hold.merchant == merchant, "Wrong escrow merchant");
        assert!(!hold.disputed, "Escrow disputed");
        assert!(
            env.ledger().timestamp() >= hold.release_at,
            "Escrow window still open"
        );
        env.storage().persistent().remove(&key);
        token::Client::new(&env, &hold.token).transfer(
            &env.current_contract_address(),
            &merchant,
            &hold.amount,
        );
    }

    pub fn get_escrow(env: Env, pull_id: u64) -> Option<EscrowHold> {
        env.storage()
            .persistent()
            .get(&SubscriptionKey::Escrow(pull_id))
    }

    /// Read the current terms for a user/merchant vault.
    /// This entrypoint requires no authorization and does not modify storage.
    pub fn get_vault(env: Env, user: Address, merchant: Address) -> Option<VaultData> {
        load_vault(&env, &vault_key(&user, &merchant))
    }

    /// Close a vault so the merchant can no longer pull from it.
    /// Only the user can cancel. The token allowance is left untouched.
    /// Removes the fixed-interval vault and/or the metered vault (#46) for the
    /// pair; panics if neither exists.
    pub fn cancel_vault(env: Env, user: Address, merchant: Address) {
        user.require_auth();

        let key = vault_key(&user, &merchant);
        let metered_key = MeteredVaultKey::Vault(user.clone(), merchant.clone());

        let has_fixed = env.storage().persistent().has(&key);
        let has_metered = env.storage().persistent().has(&metered_key);
        assert!(has_fixed || has_metered, "Vault does not exist");

        if has_fixed {
            let vault: VaultData = env.storage().persistent().get(&key).expect("Vault missing");
            deactivate_badge(&env, &user, &merchant, vault.interval_seconds);
            env.storage().persistent().remove(&key);
            env.storage()
                .persistent()
                .remove(&SubscriptionKey::Swap(user.clone(), merchant.clone()));
            env.storage()
                .persistent()
                .remove(&SubscriptionKey::EscrowPolicy(
                    user.clone(),
                    merchant.clone(),
                ));
        }
        if has_metered {
            env.storage().persistent().remove(&metered_key);
            env.storage()
                .persistent()
                .remove(&FeatureKey::Elasticity(user.clone(), merchant.clone()));
        }
        clear_recovery_state(&env, &user, &merchant);

        // Emit contract event
        env.events()
            .publish((Symbol::new(&env, "vault_cancelled"), user, merchant), ());
    }

    pub fn merchant_cancel_vault(env: Env, merchant: Address, user: Address) {
        merchant.require_auth();
        let key = VaultKey {
            user: user.clone(),
            merchant: merchant.clone(),
        };
        assert!(env.storage().persistent().has(&key), "Vault does not exist");
        env.storage().persistent().remove(&key);
        clear_recovery_state(&env, &user, &merchant);
        env.events().publish(
            (
                Symbol::new(&env, "merchant_vault_cancelled"),
                user,
                merchant,
            ),
            (),
        );
    }

    pub fn refund_subscriber(
        env: Env,
        merchant: Address,
        user: Address,
        token: Address,
        refund_amount: i128,
    ) {
        merchant.require_auth();
        assert!(refund_amount > 0, "refund_amount must be positive");
        token::Client::new(&env, &token).transfer_from(
            &env.current_contract_address(),
            &merchant,
            &user,
            &refund_amount,
        );
        env.events().publish(
            (Symbol::new(&env, "subscriber_refunded"), merchant, user),
            (token, refund_amount),
        );
    }

    pub fn record_failed_pull(env: Env, merchant: Address, user: Address) {
        merchant.require_auth();
        let key = vault_key(&user, &merchant);
        let mut vault = load_vault(&env, &key).expect("Vault does not exist");
        let current_time = env.ledger().timestamp();
        let due_date = vault
            .last_pull_timestamp
            .checked_add(vault.interval_seconds)
            .expect("Due date overflow");

        if vault.payment_status == PAYMENT_DEFAULTED {
            return;
        }
        if current_time > due_date {
            let grace_end = due_date
                .checked_add(vault.grace_period_seconds)
                .expect("Grace period overflow");
            vault.payment_status = if current_time >= grace_end {
                PAYMENT_DEFAULTED
            } else {
                PAYMENT_GRACE_PERIOD
            };
            save_vault(&env, &key, &vault);
            env.events().publish(
                (Symbol::new(&env, "payment_status_updated"), user, merchant),
                (vault.payment_status, current_time),
            );
        }
    }

    pub fn pause_vault(env: Env, user: Address, merchant: Address) {
        user.require_auth();

        let key = vault_key(&user, &merchant);
        let mut vault = load_vault(&env, &key).expect("Vault does not exist");
        vault.is_paused = true;
        save_vault(&env, &key, &vault);

        env.events()
            .publish((Symbol::new(&env, "vault_paused"), user, merchant), ());

        let (threshold, extend_to) = calculate_ttl_params(vault.interval_seconds);
        env.storage()
            .persistent()
            .extend_ttl(&key, threshold, extend_to);
        env.storage().instance().extend_ttl(threshold, extend_to);
    }

    pub fn update_vault_terms(
        env: Env,
        user: Address,
        merchant: Address,
        new_amount: i128,
        new_interval_seconds: u64,
    ) {
        user.require_auth();
        assert!(new_amount > 0, "new_amount must be positive");
        assert!(
            new_interval_seconds > 0,
            "new_interval_seconds must be greater than zero"
        );

        let key = vault_key(&user, &merchant);
        let mut vault = load_vault(&env, &key).expect("Vault does not exist");
        vault.amount_per_interval = new_amount;
        vault.interval_seconds = new_interval_seconds;
        save_vault(&env, &key, &vault);

        env.events().publish(
            (Symbol::new(&env, "vault_terms_updated"), user, merchant),
            (new_amount, new_interval_seconds),
        );

        let (threshold, extend_to) = calculate_ttl_params(new_interval_seconds);
        env.storage()
            .persistent()
            .extend_ttl(&key, threshold, extend_to);
        env.storage().instance().extend_ttl(threshold, extend_to);
    }

    /// Unfreeze a vault. The billing timestamp is reset to the current ledger
    /// timestamp so intervals that elapsed while paused are not back-billed
    /// retroactively (#93).
    pub fn resume_vault(env: Env, user: Address, merchant: Address) {
        user.require_auth();

        let key = vault_key(&user, &merchant);
        let mut vault = load_vault(&env, &key).expect("Vault does not exist");
        vault.is_paused = false;
        vault.last_pull_timestamp = env.ledger().timestamp();
        save_vault(&env, &key, &vault);

        let tier = env
            .storage()
            .persistent()
            .get::<_, BadgeData>(&SubscriptionKey::Badge(user.clone(), merchant.clone()))
            .map(|b| b.tier)
            .unwrap_or(1);
        activate_badge(
            &env,
            &user,
            &merchant,
            tier,
            env.ledger()
                .timestamp()
                .saturating_add(vault.interval_seconds),
            vault.interval_seconds,
        );

        env.events()
            .publish((Symbol::new(&env, "vault_resumed"), user, merchant), ());

        let (threshold, extend_to) = calculate_ttl_params(vault.interval_seconds);
        env.storage()
            .persistent()
            .extend_ttl(&key, threshold, extend_to);
        env.storage().instance().extend_ttl(threshold, extend_to);
    }

    /// 2. THE PULL (Execute Billing)
    /// The merchant (or the Orbit backend) calls this to pull the funds.
    pub fn pull_funds(env: Env, user: Address, merchant: Address) {
        merchant.require_auth();

        let key = vault_key(&user, &merchant);
        let mut vault = load_vault(&env, &key).expect("Vault does not exist");

        // A paused subscriber is never billed (#93)
        assert!(!vault.is_paused, "Vault is paused by subscriber");

        let current_time = env.ledger().timestamp();

        // A free trial blocks all merchant pulls until it expires (#158)
        assert!(
            current_time >= vault.trial_end_timestamp,
            "Cannot pull funds during active free trial"
        );

        let due_date = vault
            .last_pull_timestamp
            .checked_add(vault.interval_seconds)
            .expect("Due date overflow");
        if vault.payment_status == PAYMENT_GRACE_PERIOD {
            let grace_end = due_date
                .checked_add(vault.grace_period_seconds)
                .expect("Grace period overflow");
            if current_time >= grace_end {
                vault.payment_status = PAYMENT_DEFAULTED;
                env.storage().persistent().set(&key, &vault);
            }
        }
        assert!(
            vault.payment_status != PAYMENT_DEFAULTED,
            "Vault is defaulted"
        );

        if vault.last_pull_timestamp != 0 {
            assert!(
                current_time >= vault.last_pull_timestamp + vault.interval_seconds,
                "Too early to pull funds"
            );
        }

        // Deterministic epoch guard (#94): every settlement must advance the
        // billing cycle index, so no two pulls can ever land in the same cycle
        // regardless of ledger clock adjustments or retry races.
        let cycle_index =
            current_time.saturating_sub(vault.initial_timestamp) / vault.interval_seconds;
        assert!(
            vault.pull_sequence == PULL_SEQUENCE_UNSET || cycle_index > vault.pull_sequence,
            "Pull already settled in this billing cycle"
        );

        let policy_key = SubscriptionKey::EscrowPolicy(user.clone(), merchant.clone());
        let policy: Option<EscrowPolicy> = env.storage().persistent().get(&policy_key);
        let recipient = if policy.is_some() {
            env.current_contract_address()
        } else {
            merchant.clone()
        };
        pull_payment(&env, &user, &merchant, &vault, &recipient);
        if let Some(policy) = policy {
            let pull_id = store_escrow(&env, &user, &merchant, &vault, &policy);
            extend_key(
                &env,
                &policy_key,
                vault.interval_seconds.max(policy.window_seconds),
            );
            env.events().publish(
                (Symbol::new(&env, "escrow_held"), pull_id),
                (user.clone(), merchant.clone(), vault.amount_per_interval),
            );
        }

        let tier = env
            .storage()
            .persistent()
            .get::<_, BadgeData>(&SubscriptionKey::Badge(user.clone(), merchant.clone()))
            .map(|b| b.tier)
            .unwrap_or(1);
        activate_badge(
            &env,
            &user,
            &merchant,
            tier,
            current_time.saturating_add(vault.interval_seconds),
            vault.interval_seconds,
        );

        vault.last_pull_timestamp = current_time;
        vault.payment_status = PAYMENT_ACTIVE;
        vault.pull_sequence = cycle_index;
        let amount_per_interval = vault.amount_per_interval;
        let interval_seconds = vault.interval_seconds;
        save_vault(&env, &key, &vault);

        // Emit contract event
        env.events().publish(
            (Symbol::new(&env, "funds_pulled"), user, merchant),
            (amount_per_interval, current_time),
        );
        // Extend TTL on each pull for both the vault entry and the contract instance
        let (threshold, extend_to) = calculate_ttl_params(interval_seconds);
        env.storage()
            .persistent()
            .extend_ttl(&key, threshold, extend_to);
        env.storage().instance().extend_ttl(threshold, extend_to);
    }

    // --------------------------------------------------------------------
    // BATCH PULL (#92)
    // --------------------------------------------------------------------

    /// Settle many due subscribers in one transaction. Instead of submitting a
    /// separate `pull_funds` transaction per subscriber (which bloats ledger fees
    /// and contends on the merchant sequence number), the merchant authorizes one
    /// call and receives one `BatchPullResult` per subscriber.
    ///
    /// Fault tolerance: each subscriber is attempted independently. A subscriber
    /// that is ineligible (missing/paused/defaulted vault, interval not elapsed,
    /// cycle already settled) or underfunded is recorded with `success == false`
    /// instead of reverting the transfers that already settled earlier in the same
    /// batch.
    pub fn batch_pull_funds(
        env: Env,
        merchant: Address,
        subscribers: Vec<Address>,
    ) -> Vec<BatchPullResult> {
        // The merchant (or the Orbit backend acting for it) authorizes the batch.
        merchant.require_auth();

        assert!(!subscribers.is_empty(), "subscribers must not be empty");
        assert!(
            subscribers.len() <= MAX_BATCH_PULL_SUBSCRIBERS,
            "batch size exceeds MAX_BATCH_PULL_SUBSCRIBERS"
        );

        let mut results = Vec::new(&env);
        let mut success_count: u32 = 0;
        let mut total_amount: i128 = 0;

        for subscriber in subscribers.iter() {
            let result = Self::try_batch_pull_subscriber(&env, &subscriber, &merchant);
            if result.success {
                success_count += 1;
                total_amount = total_amount.saturating_add(result.amount);
            }
            results.push_back(result);
        }

        // One aggregate event per batch: (merchant, success_count, total_amount).
        env.events().publish(
            (Symbol::new(&env, "batch_funds_pulled"), merchant.clone()),
            (success_count, total_amount),
        );

        results
    }

    /// Executes a single guarded pull for `(subscriber, merchant)` and reports the
    /// outcome. Returns a failing `BatchPullResult` (never panics) for any expected
    /// per-subscriber failure so the enclosing batch keeps settling.
    fn try_batch_pull_subscriber(
        env: &Env,
        subscriber: &Address,
        merchant: &Address,
    ) -> BatchPullResult {
        let skipped = |amount: i128| BatchPullResult {
            subscriber: subscriber.clone(),
            success: false,
            amount,
        };

        let key = vault_key(subscriber, merchant);
        let mut vault = match load_vault(env, &key) {
            Some(vault) => vault,
            None => return skipped(0),
        };

        // A paused subscriber is never billed (#93).
        if vault.is_paused {
            return skipped(0);
        }
        if vault.payment_status == PAYMENT_DEFAULTED {
            return skipped(0);
        }

        let current_time = env.ledger().timestamp();
        if vault.last_pull_timestamp != 0
            && current_time < vault.last_pull_timestamp + vault.interval_seconds
        {
            return skipped(0);
        }

        // Deterministic epoch guard (#94): never settle the same billing cycle twice.
        let cycle_index =
            current_time.saturating_sub(vault.initial_timestamp) / vault.interval_seconds;
        if vault.pull_sequence != PULL_SEQUENCE_UNSET && cycle_index <= vault.pull_sequence {
            return skipped(0);
        }

        // Skip a subscriber whose balance cannot cover this interval instead of
        // letting token.transfer_from abort the whole batch. This is the most
        // common benign failure (a subscriber who topped up too late).
        let token_client = token::Client::new(env, &vault.token);
        if token_client.balance(subscriber) < vault.amount_per_interval {
            return skipped(0);
        }

        // Routes through the vault's configured swap router when one exists;
        // otherwise this is a direct token.transfer_from(subscriber -> merchant).
        pull_payment(env, subscriber, merchant, &vault, merchant);

        let amount = vault.amount_per_interval;
        let interval_seconds = vault.interval_seconds;

        vault.last_pull_timestamp = current_time;
        vault.payment_status = PAYMENT_ACTIVE;
        vault.pull_sequence = cycle_index;
        save_vault(env, &key, &vault);

        let tier = env
            .storage()
            .persistent()
            .get::<_, BadgeData>(&SubscriptionKey::Badge(
                subscriber.clone(),
                merchant.clone(),
            ))
            .map(|badge| badge.tier)
            .unwrap_or(1);
        activate_badge(
            env,
            subscriber,
            merchant,
            tier,
            current_time.saturating_add(interval_seconds),
            interval_seconds,
        );

        // Keep each processed vault (and the contract instance) resident.
        let (threshold, extend_to) = calculate_ttl_params(interval_seconds);
        env.storage()
            .persistent()
            .extend_ttl(&key, threshold, extend_to);
        env.storage().instance().extend_ttl(threshold, extend_to);

        BatchPullResult {
            subscriber: subscriber.clone(),
            success: true,
            amount,
        }
    }

    // --------------------------------------------------------------------
    // METERED (USAGE-BASED) BILLING (#46)
    // --------------------------------------------------------------------

    /// Create a usage-based allowance: the merchant may pull variable amounts
    /// up to `max_amount_per_interval` in total per `interval_seconds` epoch.
    /// Re-creating replaces the terms and starts a fresh epoch.
    pub fn create_metered_vault(
        env: Env,
        user: Address,
        merchant: Address,
        token: Address,
        max_amount_per_interval: i128,
        interval_seconds: u64,
    ) {
        user.require_auth();

        assert!(
            max_amount_per_interval > 0,
            "max_amount_per_interval must be positive"
        );
        assert!(
            interval_seconds > 0,
            "interval_seconds must be greater than zero"
        );

        let key = MeteredVaultKey::Vault(user.clone(), merchant.clone());
        env.storage()
            .persistent()
            .remove(&FeatureKey::Elasticity(user.clone(), merchant.clone()));
        let vault = MeteredVaultData {
            token: token.clone(),
            max_amount_per_interval,
            interval_seconds,
            current_epoch_start: env.ledger().timestamp(),
            epoch_spent: 0,
        };
        env.storage().persistent().set(&key, &vault);

        env.events().publish(
            (Symbol::new(&env, "metered_vault_created"), user, merchant),
            (token, max_amount_per_interval, interval_seconds),
        );

        let (threshold, extend_to) = calculate_ttl_params(interval_seconds);
        env.storage()
            .persistent()
            .extend_ttl(&key, threshold, extend_to);
        env.storage().instance().extend_ttl(threshold, extend_to);
    }

    /// Create a usage-based vault with an overdraft buffer capped at 20%.
    /// The existing `create_metered_vault` remains a strict, non-elastic path.
    pub fn create_elastic_metered_vault(
        env: Env,
        user: Address,
        merchant: Address,
        token: Address,
        max_amount_per_interval: i128,
        interval_seconds: u64,
        overdraft_percentage: u32,
    ) {
        assert!(
            overdraft_percentage <= MAX_OVERDRAFT_PERCENTAGE,
            "Overdraft percentage exceeds 20%"
        );
        Self::create_metered_vault(
            env.clone(),
            user.clone(),
            merchant.clone(),
            token,
            max_amount_per_interval,
            interval_seconds,
        );
        let key = FeatureKey::Elasticity(user.clone(), merchant.clone());
        env.storage().persistent().set(
            &key,
            &ElasticityData {
                overdraft_percentage,
                carried_overdraft: 0,
                epoch_overdraft: 0,
            },
        );
        extend_feature_key(&env, &key, interval_seconds);
        env.events().publish(
            (Symbol::new(&env, "elasticity_configured"), user, merchant),
            overdraft_percentage,
        );
    }

    pub fn get_metered_elasticity(
        env: Env,
        user: Address,
        merchant: Address,
    ) -> Option<ElasticityData> {
        env.storage()
            .persistent()
            .get(&FeatureKey::Elasticity(user, merchant))
    }

    /// Read a metered vault. No authorization and no storage mutation.
    pub fn get_metered_vault(
        env: Env,
        user: Address,
        merchant: Address,
    ) -> Option<MeteredVaultData> {
        let key = MeteredVaultKey::Vault(user, merchant);
        env.storage().persistent().get(&key)
    }

    /// Pull a variable `amount` against the current epoch's allowance. Several
    /// partial pulls per epoch are allowed until `max_amount_per_interval` is
    /// reached; the allowance resets once `interval_seconds` has elapsed since
    /// the epoch started.
    pub fn pull_metered_funds(env: Env, user: Address, merchant: Address, amount: i128) {
        merchant.require_auth();

        assert!(amount > 0, "amount must be positive");

        let key = MeteredVaultKey::Vault(user.clone(), merchant.clone());
        let mut vault: MeteredVaultData = env
            .storage()
            .persistent()
            .get(&key)
            .expect("Metered vault does not exist");
        let elasticity_key = FeatureKey::Elasticity(user.clone(), merchant.clone());
        let mut elasticity: Option<ElasticityData> =
            env.storage().persistent().get(&elasticity_key);

        // Epoch rollover: strict vaults reset to their baseline. Elastic vaults
        // carry only the overdraft used in the completed epoch into the next
        // epoch's adjusted baseline.
        let now = env.ledger().timestamp();
        if now
            >= vault
                .current_epoch_start
                .saturating_add(vault.interval_seconds)
        {
            if let Some(config) = elasticity.as_mut() {
                let elapsed =
                    now.saturating_sub(vault.current_epoch_start) / vault.interval_seconds;
                config.carried_overdraft = if elapsed > 1 {
                    // At least one full, zero-spend epoch followed the debt
                    // repayment epoch, so no overdraft remains outstanding.
                    0
                } else {
                    config.epoch_overdraft
                };
                config.epoch_overdraft = 0;
            }
            vault.current_epoch_start = now;
            vault.epoch_spent = 0;
        }

        let new_spent = vault
            .epoch_spent
            .checked_add(amount)
            .expect("Interval spending limit exceeded");
        if let Some(config) = elasticity.as_mut() {
            let overdraft_limit = vault
                .max_amount_per_interval
                .checked_mul(i128::from(config.overdraft_percentage))
                .expect("Overdraft limit overflow")
                / 100;
            let adjusted_baseline = vault
                .max_amount_per_interval
                .checked_sub(config.carried_overdraft)
                .expect("Invalid carried overdraft");
            let combined_cap = adjusted_baseline
                .checked_add(overdraft_limit)
                .expect("Interval spending limit overflow");
            assert!(
                new_spent <= combined_cap,
                "Interval spending limit exceeded"
            );
            config.epoch_overdraft = new_spent.saturating_sub(adjusted_baseline);
            assert!(
                config.epoch_overdraft <= overdraft_limit,
                "Overdraft limit exceeded"
            );
        } else {
            assert!(
                new_spent <= vault.max_amount_per_interval,
                "Interval spending limit exceeded"
            );
        }

        // Record the spend before moving funds (checks-effects-interactions).
        vault.epoch_spent = new_spent;
        env.storage().persistent().set(&key, &vault);
        if let Some(config) = elasticity {
            env.storage().persistent().set(&elasticity_key, &config);
            extend_feature_key(&env, &elasticity_key, vault.interval_seconds);
        }

        let token_client = token::Client::new(&env, &vault.token);
        token_client.transfer_from(&env.current_contract_address(), &user, &merchant, &amount);

        env.events().publish(
            (Symbol::new(&env, "metered_funds_pulled"), user, merchant),
            (amount, vault.epoch_spent, vault.max_amount_per_interval),
        );

        let (threshold, extend_to) = calculate_ttl_params(vault.interval_seconds);
        env.storage()
            .persistent()
            .extend_ttl(&key, threshold, extend_to);
        env.storage().instance().extend_ttl(threshold, extend_to);
    }

    pub fn create_stream(
        env: Env,
        sender: Address,
        recipient: Address,
        token: Address,
        deposit_amount: i128,
        start_time: u64,
        stop_time: u64,
    ) -> u64 {
        sender.require_auth();
        assert!(deposit_amount > 0, "deposit_amount must be positive");
        assert!(stop_time > start_time, "stop_time must be after start_time");

        let stream_id: u64 = env
            .storage()
            .instance()
            .get(&StorageKey::NextStreamId)
            .unwrap_or(0);
        env.storage()
            .instance()
            .set(&StorageKey::NextStreamId, &(stream_id + 1));

        token::Client::new(&env, &token).transfer_from(
            &env.current_contract_address(),
            &sender,
            &env.current_contract_address(),
            &deposit_amount,
        );
        let key = StreamKey { id: stream_id };
        env.storage().persistent().set(
            &key,
            &StreamData {
                sender: sender.clone(),
                recipient: recipient.clone(),
                token: token.clone(),
                deposit_amount,
                start_time,
                stop_time,
                amount_withdrawn: 0,
            },
        );
        env.events().publish(
            (Symbol::new(&env, "stream_created"), sender, recipient),
            (stream_id, deposit_amount, start_time, stop_time),
        );
        stream_id
    }

    pub fn withdraw_from_stream(env: Env, stream_id: u64, amount: i128) {
        let key = StreamKey { id: stream_id };
        let mut stream: StreamData = env
            .storage()
            .persistent()
            .get(&key)
            .expect("Stream does not exist");
        stream.recipient.require_auth();
        assert!(amount > 0, "amount must be positive");
        let vested = Self::vested_amount(&env, &stream);
        let available = vested - stream.amount_withdrawn;
        assert!(amount <= available, "Amount exceeds vested balance");
        stream.amount_withdrawn = stream
            .amount_withdrawn
            .checked_add(amount)
            .expect("Withdrawn amount overflow");
        token::Client::new(&env, &stream.token).transfer(
            &env.current_contract_address(),
            &stream.recipient,
            &amount,
        );
        env.storage().persistent().set(&key, &stream);
    }

    pub fn cancel_stream(env: Env, stream_id: u64) {
        let key = StreamKey { id: stream_id };
        let stream: StreamData = env
            .storage()
            .persistent()
            .get(&key)
            .expect("Stream does not exist");
        stream.sender.require_auth();
        stream.recipient.require_auth();
        let vested = Self::vested_amount(&env, &stream);
        let recipient_amount = vested - stream.amount_withdrawn;
        let sender_amount = stream.deposit_amount - vested;
        let token_client = token::Client::new(&env, &stream.token);
        if recipient_amount > 0 {
            token_client.transfer(
                &env.current_contract_address(),
                &stream.recipient,
                &recipient_amount,
            );
        }
        if sender_amount > 0 {
            token_client.transfer(
                &env.current_contract_address(),
                &stream.sender,
                &sender_amount,
            );
        }
        env.storage().persistent().remove(&key);
    }

    fn vested_amount(env: &Env, stream: &StreamData) -> i128 {
        let now = env.ledger().timestamp();
        if now <= stream.start_time {
            return 0;
        }
        if now >= stream.stop_time {
            return stream.deposit_amount;
        }
        let elapsed = now - stream.start_time;
        let duration = stream.stop_time - stream.start_time;
        stream
            .deposit_amount
            .checked_mul(elapsed as i128)
            .expect("Vested amount overflow")
            / duration as i128
    }

    /// 3. THE SPLIT (Batch Payroll Disbursement)
    /// The merchant uploads a CSV of contractors. This function takes that array
    /// and routes the stablecoins to everyone globally in a single transaction block.
    pub fn batch_disburse(env: Env, sender: Address, token: Address, splits: Vec<PaymentSplit>) {
        // The sender (e.g., the Agency Owner) MUST sign this transaction to approve the payroll.
        sender.require_auth();

        let total_amount = validate_splits(&splits);
        let recipient_count = splits.len();
        disburse(&env, &sender, &token, splits);

        // Emit contract event
        env.events().publish(
            (Symbol::new(&env, "batch_disbursed"), sender),
            (token, recipient_count, total_amount),
        );
    }

    /// 4. MULTI-SIG PAYROLL: PROPOSE
    /// The sender stages a batch disbursement that only executes once
    /// `required_approvals` distinct addresses from `approvers` approve it.
    /// Returns the proposal id.
    pub fn propose_batch_disburse(
        env: Env,
        sender: Address,
        token: Address,
        splits: Vec<PaymentSplit>,
        required_approvals: u32,
        approvers: Vec<Address>,
    ) -> u64 {
        sender.require_auth();

        let total_amount = validate_splits(&splits);

        assert!(!approvers.is_empty(), "approvers must not be empty");
        assert!(
            approvers.len() <= MAX_PAYROLL_APPROVERS,
            "too many approvers"
        );
        for (i, approver) in approvers.iter().enumerate() {
            assert!(
                approvers.first_index_of(&approver) == Some(i as u32),
                "approvers must be distinct"
            );
        }
        assert!(
            required_approvals > 0 && required_approvals <= approvers.len(),
            "required_approvals must be between 1 and the number of approvers"
        );

        let id: u64 = env
            .storage()
            .instance()
            .get(&DataKey::NextProposalId)
            .unwrap_or(0);
        env.storage()
            .instance()
            .set(&DataKey::NextProposalId, &(id + 1));

        let proposal = PayrollProposal {
            id,
            sender: sender.clone(),
            token: token.clone(),
            splits,
            approvers,
            approvals: Vec::new(&env),
            required_approvals,
            executed: false,
        };
        save_proposal(&env, &proposal);

        env.events().publish(
            (Symbol::new(&env, "payroll_proposed"), sender, id),
            (
                token,
                required_approvals,
                proposal.approvers.len(),
                total_amount,
            ),
        );
        id
    }

    /// 5. MULTI-SIG PAYROLL: APPROVE
    /// An address from the proposal's approver list signs off on it. Each approver
    /// counts once.
    pub fn approve_payroll_proposal(env: Env, approver: Address, proposal_id: u64) {
        approver.require_auth();

        let mut proposal = load_proposal(&env, proposal_id);
        assert!(!proposal.executed, "Proposal already executed");
        assert!(
            proposal.approvers.contains(&approver),
            "Address is not an approver for this proposal"
        );
        assert!(
            !proposal.approvals.contains(&approver),
            "Approver has already approved this proposal"
        );

        proposal.approvals.push_back(approver.clone());
        save_proposal(&env, &proposal);

        env.events().publish(
            (Symbol::new(&env, "payroll_approved"), approver, proposal_id),
            (proposal.approvals.len(), proposal.required_approvals),
        );
    }

    /// 6. MULTI-SIG PAYROLL: EXECUTE
    /// Once the approval threshold is met, anyone may trigger the transfers. A proposal
    /// executes at most once.
    pub fn execute_payroll_proposal(env: Env, proposal_id: u64) {
        let mut proposal = load_proposal(&env, proposal_id);
        assert!(!proposal.executed, "Proposal already executed");
        assert!(
            proposal.approvals.len() >= proposal.required_approvals,
            "Approval threshold not met"
        );

        // Marked before transferring; a failed transfer reverts this along with everything else
        proposal.executed = true;
        save_proposal(&env, &proposal);

        let total_amount = validate_splits(&proposal.splits);
        let recipient_count = proposal.splits.len();
        disburse(
            &env,
            &proposal.sender,
            &proposal.token,
            proposal.splits.clone(),
        );

        env.events().publish(
            (
                Symbol::new(&env, "payroll_executed"),
                proposal.sender,
                proposal_id,
            ),
            (proposal.token, recipient_count, total_amount),
        );
    }

    /// Read a payroll proposal and its approval state.
    /// This entrypoint requires no authorization and does not modify storage.
    pub fn get_payroll_proposal(env: Env, proposal_id: u64) -> Option<PayrollProposal> {
        env.storage()
            .persistent()
            .get(&DataKey::Proposal(proposal_id))
    }

    // --------------------------------------------------------------------
    // COMPETITIVE PAYROLL BOUNTIES: COMMIT-REVEAL (#212)
    // --------------------------------------------------------------------

    /// Fund a single-winner bounty with tokens locked in this contract.
    /// `proof_hash` is SHA256 of the winning raw proof bytes.
    /// The deadline is a ledger sequence, not a wall-clock timestamp.
    pub fn create_bounty(
        env: Env,
        creator: Address,
        token: Address,
        amount: i128,
        proof_hash: BytesN<32>,
        expires_at_ledger: u32,
    ) -> u64 {
        creator.require_auth();
        assert!(amount > 0, "amount must be positive");
        let now = env.ledger().sequence();
        assert!(
            expires_at_ledger > now.saturating_add(BOUNTY_REVEAL_DELAY),
            "bounty must outlive the challenge period"
        );
        // Keep the deadline within a bounded storage lifetime, so expired
        // escrows can still be reclaimed without depending on archival restore.
        assert!(
            expires_at_ledger - now <= MIN_TTL_LEDGERS,
            "bounty deadline exceeds storage lifetime"
        );
        let id: u64 = env.storage().instance().get(&DataKey::NextBountyId).unwrap_or(0);
        env.storage().instance().set(&DataKey::NextBountyId, &(id.checked_add(1).expect("bounty id overflow")));
        token::Client::new(&env, &token).transfer(&creator, &env.current_contract_address(), &amount);
        let bounty = CompetitiveBounty {
            creator: creator.clone(),
            token,
            amount,
            proof_hash,
            expires_at_ledger,
            claimed_by: None,
            cancelled: false,
        };
        let key = DataKey::Bounty(id);
        env.storage().persistent().set(&key, &bounty);
        env.storage().persistent().extend_ttl(&key, TTL_THRESHOLD_LEDGERS, MIN_TTL_LEDGERS);
        env.storage().instance().extend_ttl(TTL_THRESHOLD_LEDGERS, MIN_TTL_LEDGERS);
        env.events().publish((Symbol::new(&env, "bounty_created"), id), (creator, amount));
        id
    }

    /// Store an authenticated commitment. Only one outstanding commitment is
    /// permitted per claimant and bounty; this makes replay attempts explicit.
    pub fn commit_bounty_claim(
        env: Env,
        claimant: Address,
        bounty_id: u64,
        commitment_hash: BytesN<32>,
    ) {
        claimant.require_auth();
        let bounty: CompetitiveBounty = env.storage().persistent()
            .get(&DataKey::Bounty(bounty_id)).expect("unknown bounty");
        let now = env.ledger().sequence();
        assert!(!bounty.cancelled && bounty.claimed_by.is_none(), "bounty unavailable");
        assert!(now.saturating_add(BOUNTY_REVEAL_DELAY) <= bounty.expires_at_ledger, "too late to commit");
        let key = DataKey::BountyCommit(bounty_id, claimant.clone());
        assert!(!env.storage().persistent().has(&key), "commitment already exists");
        env.storage().persistent().set(&key, &BountyCommitment {
            digest: commitment_hash,
            ledger: now,
        });
        env.storage().persistent().extend_ttl(&key, TTL_THRESHOLD_LEDGERS, MIN_TTL_LEDGERS);
        env.events().publish((Symbol::new(&env, "bounty_committed"), bounty_id), claimant);
    }

    /// A reveal cannot execute until five *later* ledgers have closed.
    /// Both the claimant and the secret salt are part of the preimage.
    pub fn reveal_bounty_claim(
        env: Env,
        claimant: Address,
        bounty_id: u64,
        proof: Bytes,
        secret_salt: BytesN<32>,
    ) {
        claimant.require_auth();
        let key = DataKey::Bounty(bounty_id);
        let mut bounty: CompetitiveBounty =
            env.storage().persistent().get(&key).expect("unknown bounty");
        assert!(!bounty.cancelled && bounty.claimed_by.is_none(), "bounty unavailable");
        let now = env.ledger().sequence();
        assert!(now <= bounty.expires_at_ledger, "bounty expired");
        let claim_key = DataKey::BountyCommit(bounty_id, claimant.clone());
        let commitment: BountyCommitment =
            env.storage().persistent().get(&claim_key).expect("claimant did not commit");
        assert!(
            now >= commitment.ledger.saturating_add(BOUNTY_REVEAL_DELAY),
            "reveal is too early"
        );
        assert_eq!(
            bounty_claim_digest(&env, &claimant, bounty_id, &proof, &secret_salt),
            commitment.digest,
            "invalid commitment preimage"
        );
        assert_eq!(
            env.crypto().sha256(&proof).to_bytes(),
            bounty.proof_hash,
            "invalid bounty solution"
        );
        // Update before calling token; failed transfers roll back storage.
        bounty.claimed_by = Some(claimant.clone());
        env.storage().persistent().set(&key, &bounty);
        env.storage().persistent().remove(&claim_key);
        token::Client::new(&env, &bounty.token).transfer(
            &env.current_contract_address(), &claimant, &bounty.amount
        );
        env.events().publish((Symbol::new(&env, "bounty_claimed"), bounty_id),
            (claimant, bounty.amount));
    }

    /// Reclaim only when a bounty expires without a winner.
    pub fn reclaim_expired_bounty(env: Env, bounty_id: u64) {
        let key = DataKey::Bounty(bounty_id);
        let mut bounty: CompetitiveBounty =
            env.storage().persistent().get(&key).expect("unknown bounty");
        bounty.creator.require_auth();
        assert!(env.ledger().sequence() > bounty.expires_at_ledger, "bounty not expired");
        assert!(!bounty.cancelled && bounty.claimed_by.is_none(), "bounty unavailable");
        bounty.cancelled = true;
        env.storage().persistent().set(&key, &bounty);
        token::Client::new(&env, &bounty.token).transfer(
            &env.current_contract_address(), &bounty.creator, &bounty.amount
        );
    }

    pub fn get_bounty(env: Env, bounty_id: u64) -> Option<CompetitiveBounty> {
        env.storage().persistent().get(&DataKey::Bounty(bounty_id))
    }

    // --------------------------------------------------------------------
    // FUNDED MILESTONE PAYROLL AND SIGN-OFF GATES (#209)
    // --------------------------------------------------------------------

    /// Create an agreement of up to 12 stages, funding ALL stages atomically.
    /// Contractors cannot withdraw without an explicit merchant-approved release.
    pub fn create_milestone_project(
        env: Env,
        merchant: Address,
        contractor: Address,
        token: Address,
        input_stages: Vec<MilestoneInput>,
    ) -> u64 {
        merchant.require_auth();
        assert!(!input_stages.is_empty() && input_stages.len() <= 12, "invalid milestone count");
        assert!(merchant != contractor, "merchant and contractor must differ");
        let mut stages = Vec::<MilestoneStage>::new(&env);
        let mut total = 0i128;
        for input in input_stages.iter() {
            assert!(input.title.len() > 0 && input.title.len() <= 120, "invalid title");
            assert!(input.deliverable_url.len() <= 512, "deliverable URL too long");
            assert!(input.amount > 0, "milestone amount must be positive");
            assert!(input.due_timestamp > env.ledger().timestamp(), "milestone due date must be in future");
            total = total.checked_add(input.amount).expect("milestone amount overflow");
            stages.push_back(MilestoneStage {
                title: input.title,
                deliverable_url: input.deliverable_url,
                amount: input.amount,
                due_timestamp: input.due_timestamp,
                proof_url: String::from_str(&env, ""),
                review_feedback: String::from_str(&env, ""),
                submitted: false,
                released: false,
            });
        }
        let id: u64 = env.storage().instance().get(&DataKey::NextMilestoneProjectId).unwrap_or(0);
        env.storage().instance().set(&DataKey::NextMilestoneProjectId,
            &id.checked_add(1).expect("project id overflow"));
        token::Client::new(&env, &token).transfer(
            &merchant, &env.current_contract_address(), &total
        );
        let project = MilestoneProject {
            id, merchant: merchant.clone(), contractor: contractor.clone(), token,
            escrowed: total, released_amount: 0, stages,
        };
        let key = DataKey::MilestoneProject(id);
        env.storage().persistent().set(&key, &project);
        env.storage().persistent().extend_ttl(&key, TTL_THRESHOLD_LEDGERS, MIN_TTL_LEDGERS);
        env.storage().instance().extend_ttl(TTL_THRESHOLD_LEDGERS, MIN_TTL_LEDGERS);
        env.events().publish((Symbol::new(&env, "milestones_created"), id), (merchant, contractor, total));
        id
    }

    /// The registered contractor submits proof; merchant remains the only
    /// actor who can move escrowed funds. A revised proof reopens review.
    pub fn submit_milestone_deliverable(
        env: Env, contractor: Address, project_id: u64, stage_index: u32, proof_url: String
    ) {
        contractor.require_auth();
        assert!(proof_url.len() > 0 && proof_url.len() <= 512, "invalid proof URL");
        let key = DataKey::MilestoneProject(project_id);
        let mut project: MilestoneProject = env.storage().persistent().get(&key).expect("unknown project");
        assert_eq!(project.contractor, contractor, "only assigned contractor may submit");
        let mut stage = project.stages.get(stage_index).expect("unknown milestone");
        assert!(!stage.released, "milestone already released");
        stage.proof_url = proof_url;
        stage.review_feedback = String::from_str(&env, "");
        stage.submitted = true;
        project.stages.set(stage_index, stage);
        env.storage().persistent().set(&key, &project);
        env.events().publish((Symbol::new(&env, "milestone_submitted"), project_id), stage_index);
    }

    /// Merchant feedback explicitly blocks release until a new proof is submitted.
    pub fn request_milestone_changes(
        env: Env, merchant: Address, project_id: u64, stage_index: u32, feedback: String
    ) {
        merchant.require_auth();
        assert!(feedback.len() > 0 && feedback.len() <= 512, "invalid review feedback");
        let key = DataKey::MilestoneProject(project_id);
        let mut project: MilestoneProject = env.storage().persistent().get(&key).expect("unknown project");
        assert_eq!(project.merchant, merchant, "only project merchant may review");
        let mut stage = project.stages.get(stage_index).expect("unknown milestone");
        assert!(!stage.released && stage.submitted, "milestone not under review");
        stage.submitted = false;
        stage.review_feedback = feedback;
        project.stages.set(stage_index, stage);
        env.storage().persistent().set(&key, &project);
        env.events().publish((Symbol::new(&env, "milestone_changes"), project_id), stage_index);
    }

    /// Freighter-signed merchant authorization releases exactly one stage.
    /// State changes and token transfer are atomic; a failed transfer rolls back.
    pub fn approve_and_release_milestone(
        env: Env, merchant: Address, project_id: u64, stage_index: u32
    ) {
        merchant.require_auth();
        let key = DataKey::MilestoneProject(project_id);
        let mut project: MilestoneProject = env.storage().persistent().get(&key).expect("unknown project");
        assert_eq!(project.merchant, merchant, "only merchant may approve");
        let mut stage = project.stages.get(stage_index).expect("unknown milestone");
        assert!(stage.submitted && !stage.released, "milestone is not awaiting approval");
        // Enforce release order so later milestone approvals cannot bypass earlier stages.
        for earlier in 0..stage_index {
            assert!(project.stages.get(earlier).expect("earlier stage missing").released,
                "earlier milestone must be released first");
        }
        stage.released = true;
        let amount = stage.amount;
        project.stages.set(stage_index, stage);
        project.released_amount = project.released_amount.checked_add(amount).expect("release overflow");
        assert!(project.released_amount <= project.escrowed, "escrow budget exceeded");
        env.storage().persistent().set(&key, &project);
        token::Client::new(&env, &project.token).transfer(
            &env.current_contract_address(), &project.contractor, &amount
        );
        env.events().publish(
            (Symbol::new(&env, "milestone_released"), project_id, stage_index),
            (project.contractor, amount),
        );
    }

    pub fn get_milestone_project(env: Env, project_id: u64) -> Option<MilestoneProject> {
        env.storage().persistent().get(&DataKey::MilestoneProject(project_id))
    }

    // --------------------------------------------------------------------
    // PAYROLL ESCROW & UNCLAIMED RECLAMATION (#188)
    // --------------------------------------------------------------------

    /// Deposit `amount` of `token` into contract escrow for `recipient`. The
    /// recipient may claim until `expiration_timestamp`; after that the sender can
    /// reclaim whatever was never claimed. Returns the new payroll id.
    pub fn create_payroll_escrow(
        env: Env,
        sender: Address,
        recipient: Address,
        token: Address,
        amount: i128,
        expiration_timestamp: u64,
    ) -> u64 {
        sender.require_auth();
        assert!(amount > 0, "amount must be positive");
        let now = env.ledger().timestamp();
        assert!(
            expiration_timestamp > now,
            "expiration_timestamp must be in the future"
        );

        let counter_key = DataKey::NextPayrollEscrowId;
        let id: u64 = env.storage().instance().get(&counter_key).unwrap_or(0);
        env.storage().instance().set(
            &counter_key,
            &id.checked_add(1).expect("Payroll escrow ID exhausted"),
        );

        // Pull the deposit into the contract before recording it, so a failed
        // transfer reverts the whole call and leaves no record behind.
        token::Client::new(&env, &token).transfer_from(
            &env.current_contract_address(),
            &sender,
            &env.current_contract_address(),
            &amount,
        );

        let escrow = PayrollEscrow {
            id,
            sender: sender.clone(),
            recipient: recipient.clone(),
            token: token.clone(),
            total_deposited: amount,
            claimed_amount: 0,
            expiration_timestamp,
            reclaimed: false,
        };
        save_payroll_escrow(&env, &escrow);

        env.events().publish(
            (Symbol::new(&env, "payroll_escrow_created"), sender, id),
            (recipient, token, amount, expiration_timestamp),
        );
        id
    }

    /// Recipient withdraws up to the unclaimed balance. Claims are only accepted
    /// while `now <= expiration_timestamp`.
    pub fn claim_payroll_escrow(env: Env, recipient: Address, payroll_id: u64, amount: i128) {
        recipient.require_auth();
        assert!(amount > 0, "amount must be positive");

        let mut escrow = load_payroll_escrow(&env, payroll_id);
        assert!(escrow.recipient == recipient, "Not the payroll recipient");
        assert!(!escrow.reclaimed, "Payroll escrow already reclaimed");
        assert!(
            env.ledger().timestamp() <= escrow.expiration_timestamp,
            "Claim window has expired"
        );
        let unclaimed = escrow.total_deposited - escrow.claimed_amount;
        assert!(amount <= unclaimed, "amount exceeds unclaimed balance");

        escrow.claimed_amount += amount;
        save_payroll_escrow(&env, &escrow);

        token::Client::new(&env, &escrow.token).transfer(
            &env.current_contract_address(),
            &recipient,
            &amount,
        );

        env.events().publish(
            (
                Symbol::new(&env, "payroll_escrow_claimed"),
                recipient,
                payroll_id,
            ),
            (escrow.token, amount),
        );
    }

    /// Returns the unclaimed balance of a payroll escrow to its sender once the
    /// claim window has closed (`now > expiration_timestamp`). The escrow is marked
    /// reclaimed before any transfer, so it can only ever be reclaimed once.
    pub fn reclaim_unclaimed_payroll(env: Env, sender: Address, payroll_id: u64) -> i128 {
        sender.require_auth();

        let mut escrow = load_payroll_escrow(&env, payroll_id);
        assert!(escrow.sender == sender, "Not the payroll sender");
        assert!(!escrow.reclaimed, "Payroll escrow already reclaimed");
        assert!(
            env.ledger().timestamp() > escrow.expiration_timestamp,
            "Expiration timestamp has not passed"
        );

        let unclaimed = escrow.total_deposited - escrow.claimed_amount;
        assert!(unclaimed > 0, "No unclaimed balance to reclaim");

        escrow.reclaimed = true;
        save_payroll_escrow(&env, &escrow);

        token::Client::new(&env, &escrow.token).transfer(
            &env.current_contract_address(),
            &sender,
            &unclaimed,
        );

        env.events().publish(
            (
                Symbol::new(&env, "payroll_escrow_reclaimed"),
                sender,
                payroll_id,
            ),
            (escrow.token, unclaimed),
        );
        unclaimed
    }

    /// Read a payroll escrow record. Requires no authorization.
    pub fn get_payroll_escrow(env: Env, payroll_id: u64) -> Option<PayrollEscrow> {
        env.storage()
            .persistent()
            .get(&DataKey::PayrollEscrow(payroll_id))
    }

    // --------------------------------------------------------------------
    // MERKLE PAYROLL DISTRIBUTION (#156)
    // --------------------------------------------------------------------

    /// Escrow `total_amount` of `token` for a payroll described by
    /// `merkle_root`, whose leaves are `SHA256(recipient_xdr || amount_be)`.
    /// Recipients claim until `now + duration_seconds`. Returns the claim id.
    pub fn create_merkle_payroll(
        env: Env,
        sender: Address,
        token: Address,
        merkle_root: BytesN<32>,
        total_amount: i128,
        duration_seconds: u64,
    ) -> u64 {
        sender.require_auth();
        assert!(total_amount > 0, "amount must be positive");
        assert!(duration_seconds > 0, "duration must be positive");
        let expiration_timestamp = env
            .ledger()
            .timestamp()
            .checked_add(duration_seconds)
            .expect("expiration overflow");

        let counter_key = DataKey::NextMerklePayrollId;
        let id: u64 = env.storage().instance().get(&counter_key).unwrap_or(0);
        env.storage().instance().set(
            &counter_key,
            &id.checked_add(1).expect("Merkle payroll ID exhausted"),
        );

        // Lock the whole payroll before publishing the root, so a failed
        // transfer reverts the call and leaves no claimable distribution.
        token::Client::new(&env, &token).transfer_from(
            &env.current_contract_address(),
            &sender,
            &env.current_contract_address(),
            &total_amount,
        );

        let payroll = MerklePayrollData {
            sender: sender.clone(),
            token: token.clone(),
            merkle_root: merkle_root.clone(),
            total_deposited: total_amount,
            claimed_amount: 0,
            expiration_timestamp,
        };
        save_merkle_payroll(&env, id, &payroll);

        env.events().publish(
            (Symbol::new(&env, "merkle_payroll_created"), sender, id),
            (token, merkle_root, total_amount, expiration_timestamp),
        );
        id
    }

    /// Pay `amount` to `recipient` if `proof` shows `(recipient, amount)` is a
    /// leaf of the payroll's root. Permissionless: funds only ever go to the
    /// proven recipient, so a relayer may submit the claim on their behalf.
    /// Each leaf can be claimed once.
    pub fn claim_merkle_payroll(
        env: Env,
        claim_id: u64,
        recipient: Address,
        amount: i128,
        proof: Vec<BytesN<32>>,
    ) {
        assert!(amount > 0, "amount must be positive");
        let mut payroll = load_merkle_payroll(&env, claim_id);
        assert!(
            env.ledger().timestamp() <= payroll.expiration_timestamp,
            "Claim window has expired"
        );

        let leaf = merkle_leaf(&env, &recipient, amount);
        assert!(
            verify_merkle_proof(&env, &proof, &payroll.merkle_root, &leaf),
            "Invalid Merkle proof"
        );

        let claimed_key = DataKey::MerkleClaimed(claim_id, leaf);
        assert!(
            !env.storage().persistent().has(&claimed_key),
            "Leaf already claimed"
        );
        let claimed_amount = payroll
            .claimed_amount
            .checked_add(amount)
            .expect("claimed amount overflow");
        assert!(
            claimed_amount <= payroll.total_deposited,
            "amount exceeds unclaimed balance"
        );

        // Record the claim before transferring, so the leaf can never pay twice
        payroll.claimed_amount = claimed_amount;
        save_merkle_payroll(&env, claim_id, &payroll);
        env.storage().persistent().set(&claimed_key, &true);
        let (threshold, extend_to) = calculate_ttl_params(
            payroll
                .expiration_timestamp
                .saturating_sub(env.ledger().timestamp()),
        );
        env.storage()
            .persistent()
            .extend_ttl(&claimed_key, threshold, extend_to);

        token::Client::new(&env, &payroll.token).transfer(
            &env.current_contract_address(),
            &recipient,
            &amount,
        );

        env.events().publish(
            (
                Symbol::new(&env, "merkle_payroll_claimed"),
                recipient,
                claim_id,
            ),
            (payroll.token, amount),
        );
    }

    /// Returns the unclaimed remainder of a Merkle payroll to its sender once
    /// the claim window has closed. Marks the payroll fully settled first, so
    /// it can only be reclaimed once.
    pub fn reclaim_merkle_payroll(env: Env, sender: Address, claim_id: u64) -> i128 {
        sender.require_auth();
        let mut payroll = load_merkle_payroll(&env, claim_id);
        assert!(payroll.sender == sender, "Not the payroll sender");
        assert!(
            env.ledger().timestamp() > payroll.expiration_timestamp,
            "Expiration timestamp has not passed"
        );
        let unclaimed = payroll.total_deposited - payroll.claimed_amount;
        assert!(unclaimed > 0, "No unclaimed balance to reclaim");

        payroll.claimed_amount = payroll.total_deposited;
        save_merkle_payroll(&env, claim_id, &payroll);

        token::Client::new(&env, &payroll.token).transfer(
            &env.current_contract_address(),
            &sender,
            &unclaimed,
        );

        env.events().publish(
            (
                Symbol::new(&env, "merkle_payroll_reclaimed"),
                sender,
                claim_id,
            ),
            (payroll.token, unclaimed),
        );
        unclaimed
    }

    /// Read a Merkle payroll distribution. Requires no authorization.
    pub fn get_merkle_payroll(env: Env, claim_id: u64) -> Option<MerklePayrollData> {
        env.storage()
            .persistent()
            .get(&DataKey::MerklePayroll(claim_id))
    }

    /// Whether the `(recipient, amount)` leaf of a Merkle payroll was claimed.
    pub fn is_merkle_leaf_claimed(
        env: Env,
        claim_id: u64,
        recipient: Address,
        amount: i128,
    ) -> bool {
        let leaf = merkle_leaf(&env, &recipient, amount);
        env.storage()
            .persistent()
            .has(&DataKey::MerkleClaimed(claim_id, leaf))
    }

    // --------------------------------------------------------------------
    // ON-CHAIN PLAN CATALOG (#95)
    // --------------------------------------------------------------------

    /// Register a plan in the public on-chain catalog under merchant authority.
    /// Re-registering an existing `plan_id` is only allowed for the merchant
    /// that already owns it (it refreshes the terms and reactivates the plan).
    pub fn register_plan(
        env: Env,
        merchant: Address,
        plan_id: String,
        token: Address,
        amount_per_interval: i128,
        interval_seconds: u64,
        name: String,
    ) {
        merchant.require_auth();

        assert!(
            amount_per_interval > 0,
            "amount_per_interval must be positive"
        );
        assert!(
            interval_seconds > 0,
            "interval_seconds must be greater than zero"
        );

        let key = PlanKey {
            plan_id: plan_id.clone(),
        };
        if let Some(existing) = env.storage().persistent().get::<PlanKey, PlanData>(&key) {
            assert!(
                existing.merchant == merchant,
                "Plan ID is already registered by another merchant"
            );
        }

        let plan = PlanData {
            merchant: merchant.clone(),
            token: token.clone(),
            amount_per_interval,
            interval_seconds,
            name: name.clone(),
            is_active: true,
        };
        env.storage().persistent().set(&key, &plan);

        env.events().publish(
            (Symbol::new(&env, "plan_registered"), merchant, plan_id),
            (token, amount_per_interval, interval_seconds, name),
        );

        let (threshold, extend_to) = calculate_ttl_params(interval_seconds);
        env.storage()
            .persistent()
            .extend_ttl(&key, threshold, extend_to);
        env.storage().instance().extend_ttl(threshold, extend_to);
    }

    /// Disable new subscriptions for a plan. Existing vaults created from the
    /// plan keep running, so in-flight subscribers are unaffected (#95).
    pub fn deactivate_plan(env: Env, merchant: Address, plan_id: String) {
        merchant.require_auth();

        let key = PlanKey {
            plan_id: plan_id.clone(),
        };
        let mut plan: PlanData = env
            .storage()
            .persistent()
            .get(&key)
            .expect("Plan does not exist");
        assert!(
            plan.merchant == merchant,
            "Only the owning merchant can deactivate this plan"
        );

        plan.is_active = false;
        env.storage().persistent().set(&key, &plan);

        env.events().publish(
            (Symbol::new(&env, "plan_deactivated"), merchant, plan_id),
            (),
        );

        let (threshold, extend_to) = calculate_ttl_params(plan.interval_seconds);
        env.storage()
            .persistent()
            .extend_ttl(&key, threshold, extend_to);
        env.storage().instance().extend_ttl(threshold, extend_to);
    }

    /// Public read helper for the on-chain plan catalog. No authorization and
    /// no storage mutation.
    pub fn get_plan(env: Env, plan_id: String) -> Option<PlanData> {
        let key = PlanKey { plan_id };
        env.storage().persistent().get(&key)
    }

    /// Create a vault using terms read directly from the on-chain catalog, so
    /// the subscriber never has to trust off-chain plan metadata. Rejects
    /// deactivated plans while leaving existing vaults untouched (#95).
    pub fn create_vault_from_plan(env: Env, user: Address, plan_id: String) {
        user.require_auth();

        let key = PlanKey { plan_id };
        let plan: PlanData = env
            .storage()
            .persistent()
            .get(&key)
            .expect("Plan does not exist");
        assert!(plan.is_active, "Plan is not active");

        write_vault(
            &env,
            &user,
            &plan.merchant,
            &plan.token,
            plan.amount_per_interval,
            plan.interval_seconds,
            DEFAULT_GRACE_PERIOD_SECONDS,
            0,
        );
    }

    // --------------------------------------------------------------------
    // YIELD-BEARING COLLATERAL STRATEGY (#176)
    // --------------------------------------------------------------------

    /// Configure an automated yield strategy for idle contract escrow. When
    /// escrow balances exceed the `liquidity_buffer`, excess tokens are
    /// automatically deployed to the approved lending pool to generate yield.
    pub fn configure_yield_strategy(
        env: Env,
        admin: Address,
        yield_pool: Address,
        token: Address,
        liquidity_buffer: i128,
    ) {
        admin.require_auth();
        assert!(liquidity_buffer >= 0, "Liquidity buffer must be non-negative");

        let key = FeatureKey::YieldStrategy;
        let config = YieldStrategyConfig {
            yield_pool: Some(yield_pool.clone()),
            token: token.clone(),
            liquidity_buffer,
            total_deployed: 0,
        };
        env.storage().persistent().set(&key, &config);
        extend_feature_key(&env, &key, MIN_TTL_LEDGERS as u64 * SECONDS_PER_LEDGER);

        env.events().publish(
            (Symbol::new(&env, "yield_strategy_configured"), admin),
            (yield_pool, token, liquidity_buffer),
        );
    }

    /// Deploy excess idle capital to the configured yield market. This checks
    /// the contract's token balance against the liquidity buffer and supplies
    /// the surplus to the lending pool to earn passive interest.
    pub fn deploy_idle_capital(env: Env, token: Address) {
        let key = FeatureKey::YieldStrategy;
        let mut config: YieldStrategyConfig = env
            .storage()
            .persistent()
            .get(&key)
            .expect("Yield strategy not configured");
        assert!(config.token == token, "Wrong token for yield strategy");

        let pool_address = config
            .yield_pool
            .as_ref()
            .expect("No yield pool configured");
        let contract_addr = env.current_contract_address();
        let token_client = token::Client::new(&env, &token);
        let current_balance = token_client.balance(&contract_addr);

        let deployable = current_balance
            .checked_sub(config.liquidity_buffer)
            .unwrap_or(0);
        assert!(deployable > 0, "No excess capital to deploy");

        // Supply tokens to the lending pool
        let pool_client = LendingPoolClient::new(&env, pool_address);
        pool_client.supply(&token, &deployable, &contract_addr);

        config.total_deployed = config
            .total_deployed
            .checked_add(deployable)
            .expect("Total deployed overflow");
        env.storage().persistent().set(&key, &config);
        extend_feature_key(&env, &key, MIN_TTL_LEDGERS as u64 * SECONDS_PER_LEDGER);

        env.events().publish(
            (Symbol::new(&env, "capital_deployed"), token),
            (deployable, config.total_deployed),
        );
    }

    /// Redeem principal plus accrued interest from the yield market. This is
    /// called when merchants withdraw funds or escrow is released, ensuring
    /// available liquidity while capturing the earned yield.
    pub fn redeem_yield_principal(env: Env, token: Address, amount: i128) {
        assert!(amount > 0, "Redemption amount must be positive");

        let key = FeatureKey::YieldStrategy;
        let mut config: YieldStrategyConfig = env
            .storage()
            .persistent()
            .get(&key)
            .expect("Yield strategy not configured");
        assert!(config.token == token, "Wrong token for yield strategy");

        let pool_address = config
            .yield_pool
            .as_ref()
            .expect("No yield pool configured");
        let contract_addr = env.current_contract_address();

        // Withdraw from the lending pool (principal + interest)
        let pool_client = LendingPoolClient::new(&env, pool_address);
        let redeemed = pool_client.withdraw(&token, &amount, &contract_addr);
        assert!(redeemed >= amount, "Insufficient yield pool liquidity");

        config.total_deployed = config
            .total_deployed
            .checked_sub(amount.min(config.total_deployed))
            .unwrap_or(0);
        env.storage().persistent().set(&key, &config);
        extend_feature_key(&env, &key, MIN_TTL_LEDGERS as u64 * SECONDS_PER_LEDGER);

        let yield_earned = redeemed.saturating_sub(amount);
        env.events().publish(
            (Symbol::new(&env, "principal_redeemed"), token),
            (amount, redeemed, yield_earned),
        );
    }

    /// Emergency unwind: immediately withdraw all deployed funds from the yield
    /// market back to the base contract. This is admin-guarded and used when
    /// market utilization spikes or safety concerns require immediate liquidity.
    pub fn emergency_unwind_yield(env: Env, admin: Address) {
        admin.require_auth();

        let key = FeatureKey::YieldStrategy;
        let mut config: YieldStrategyConfig = env
            .storage()
            .persistent()
            .get(&key)
            .expect("Yield strategy not configured");

        let pool_address = config
            .yield_pool
            .as_ref()
            .expect("No yield pool configured");
        let contract_addr = env.current_contract_address();
        let pool_client = LendingPoolClient::new(&env, &config.token);

        // Query the contract's balance in the lending pool
        let pool_balance = pool_client.balance_of(&config.token, &contract_addr);
        if pool_balance > 0 {
            let pool = LendingPoolClient::new(&env, pool_address);
            let recovered = pool.withdraw(&config.token, &pool_balance, &contract_addr);

            config.total_deployed = 0;
            env.storage().persistent().set(&key, &config);

            env.events().publish(
                (Symbol::new(&env, "emergency_yield_unwind"), admin),
                (config.token, recovered),
            );
        }
    }

    /// Read the current yield strategy configuration. No authorization required.
    pub fn get_yield_strategy(env: Env) -> Option<YieldStrategyConfig> {
        env.storage()
            .persistent()
            .get(&FeatureKey::YieldStrategy)
    }
}

/// Checks every split upfront so a bad entry fails before any transfer. Returns the total.
fn validate_splits(splits: &Vec<PaymentSplit>) -> i128 {
    assert!(!splits.is_empty(), "splits must not be empty");

    let mut total_amount: i128 = 0;
    for split in splits.iter() {
        assert!(split.amount > 0, "split amount must be positive");
        total_amount = total_amount
            .checked_add(split.amount)
            .expect("Total amount overflow");
    }
    total_amount
}

/// Transfers each split from the sender's allowance to its recipient.
fn disburse(env: &Env, sender: &Address, token: &Address, splits: Vec<PaymentSplit>) {
    let token_client = token::Client::new(env, token);

    // Loop through the array of contractors and amounts
    for split in splits.into_iter() {
        // Transfer the exact cut directly from the sender's wallet to the contractor
        token_client.transfer_from(
            &env.current_contract_address(),
            sender,
            &split.recipient,
            &split.amount,
        );
    }
}

fn load_proposal(env: &Env, proposal_id: u64) -> PayrollProposal {
    env.storage()
        .persistent()
        .get(&DataKey::Proposal(proposal_id))
        .expect("Proposal does not exist")
}

fn load_payroll_escrow(env: &Env, payroll_id: u64) -> PayrollEscrow {
    env.storage()
        .persistent()
        .get(&DataKey::PayrollEscrow(payroll_id))
        .expect("Payroll escrow does not exist")
}

/// Persists a payroll escrow and keeps it alive until its expiration (plus the
/// standard margin), so a recipient can still claim right up to the deadline.
fn save_payroll_escrow(env: &Env, escrow: &PayrollEscrow) {
    let key = DataKey::PayrollEscrow(escrow.id);
    env.storage().persistent().set(&key, escrow);
    let remaining = escrow
        .expiration_timestamp
        .saturating_sub(env.ledger().timestamp());
    let (threshold, extend_to) = calculate_ttl_params(remaining);
    env.storage()
        .persistent()
        .extend_ttl(&key, threshold, extend_to);
}

fn load_merkle_payroll(env: &Env, claim_id: u64) -> MerklePayrollData {
    env.storage()
        .persistent()
        .get(&DataKey::MerklePayroll(claim_id))
        .expect("Merkle payroll does not exist")
}

/// Persists a Merkle payroll and keeps it alive until its claim window closes.
fn save_merkle_payroll(env: &Env, claim_id: u64, payroll: &MerklePayrollData) {
    let key = DataKey::MerklePayroll(claim_id);
    env.storage().persistent().set(&key, payroll);
    let remaining = payroll
        .expiration_timestamp
        .saturating_sub(env.ledger().timestamp());
    let (threshold, extend_to) = calculate_ttl_params(remaining);
    env.storage()
        .persistent()
        .extend_ttl(&key, threshold, extend_to);
}

/// Merkle payroll leaf: `SHA256(recipient_xdr || amount as 16 big-endian bytes)`.
/// The XDR address encoding is never 64 bytes long, so a leaf preimage can't be
/// mistaken for an inner node (`left || right`).
pub fn merkle_leaf(env: &Env, recipient: &Address, amount: i128) -> BytesN<32> {
    use soroban_sdk::xdr::ToXdr;
    let mut data = recipient.clone().to_xdr(env);
    data.extend_from_array(&amount.to_be_bytes());
    env.crypto().sha256(&data).to_bytes()
}

/// Inner Merkle node: SHA256 of the two children in ascending byte order, so a
/// proof only needs the sibling hashes and not their left/right positions.
pub fn merkle_hash_pair(env: &Env, a: &BytesN<32>, b: &BytesN<32>) -> BytesN<32> {
    let (first, second) = if a.to_array() <= b.to_array() {
        (a, b)
    } else {
        (b, a)
    };
    let mut data = Bytes::from_array(env, &first.to_array());
    data.extend_from_array(&second.to_array());
    env.crypto().sha256(&data).to_bytes()
}

fn verify_merkle_proof(
    env: &Env,
    proof: &Vec<BytesN<32>>,
    root: &BytesN<32>,
    leaf: &BytesN<32>,
) -> bool {
    let mut computed = leaf.clone();
    for sibling in proof.iter() {
        computed = merkle_hash_pair(env, &computed, &sibling);
    }
    computed == *root
}

fn save_proposal(env: &Env, proposal: &PayrollProposal) {
    let key = DataKey::Proposal(proposal.id);
    env.storage().persistent().set(&key, proposal);
    // Keep pending and executed proposals live; executed ones are kept for replay protection
    env.storage()
        .persistent()
        .extend_ttl(&key, TTL_THRESHOLD_LEDGERS, MIN_TTL_LEDGERS);
    env.storage()
        .instance()
        .extend_ttl(TTL_THRESHOLD_LEDGERS, MIN_TTL_LEDGERS);
}

#[cfg(test)]
mod test;

#[cfg(test)]
mod test_fuzz;

#[cfg(test)]
mod test_storage;
