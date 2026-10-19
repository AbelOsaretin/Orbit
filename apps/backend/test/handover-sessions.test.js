import { describe, expect, it } from 'vitest';
import { createRequire } from 'module';
import {
  Address, Keypair, StrKey, nativeToScVal, xdr,
} from '@stellar/stellar-sdk';

const require = createRequire(import.meta.url);
const { createHandoverService, matchingVaultEvent, HANDOVER_TTL_MS } =
  require('../handover-sessions.js');

const merchant = Keypair.random().publicKey();
const subscriber = Keypair.random().publicKey();
const contract = StrKey.encodeContract(Buffer.alloc(32, 13));
const token = StrKey.encodeContract(Buffer.alloc(32, 22));
const txHash = 'a'.repeat(64);
const planId = 'e0b8e99b-5136-4d1d-9351-91a5db4fb056';
const subscriptionId = '4a7f2e18-6c51-41b9-9cf3-90d1bf379b32';
const plan = {
  id: planId, merchant_id: 'merchant-1', merchants: { wallet_address: merchant },
  usdc_amount: '290000000', interval_seconds: 2592000,
};
const subscription = { id: subscriptionId, plan_id: planId, customer_wallet_address: subscriber };

const event = ({ topic = 'vault_created', address = subscriber, amount = 290000000n } = {}) => ({
  contractId: () => StrKey.decodeContract(contract),
  body: () => ({ v0: () => ({
    topics: () => [
      nativeToScVal(topic, { type: 'symbol' }),
      Address.fromString(address).toScVal(),
      Address.fromString(merchant).toScVal(),
    ],
    data: () => xdr.ScVal.scvVec([
      Address.fromString(token).toScVal(),
      nativeToScVal(amount, { type: 'i128' }),
      nativeToScVal(2592000n, { type: 'u64' }),
      nativeToScVal(0n, { type: 'u64' }),
    ]),
  }) }),
});
const confirmed = (changes) => ({
  status: 'SUCCESS', ledger: 111,
  events: { contractEventsXdr: [[event(changes)]] },
});
function mockDb() {
  return {
    from(table) {
      return {
        select() { return this; },
        eq() { return this; },
        async maybeSingle() { return { data: table === 'plans' ? plan : table === 'subscriptions' ? subscription : null, error: null }; },
      };
    },
  };
}

function makeService({ clock = 1000, txn = confirmed(), db = mockDb() } = {}) {
  let time = clock;
  const service = createHandoverService({
    supabaseProvider: () => db,
    rpcUrl: 'https://rpc.example.com', contractId: contract, tokenContractId: token,
    redisUrl: '', now: () => time,
    serverFactory: () => ({
      async getLatestLedger() { return { sequence: 110 }; },
      async getTransaction() { return txn; },
    }),
  });
  return { service, tick: (duration) => { time += duration; } };
}

describe('verified five-minute cross-device handover', () => {
  it('binds an unguessable session to the plan and enforces one successful completion', async () => {
    const { service } = makeService();
    const created = await service.create(planId);
    expect(created.id).toMatch(/^[0-9a-f]{48}$/);
    expect(created.status).toBe('pending');
    expect(created.expiresAt).toBe(1000 + HANDOVER_TTL_MS);
    expect(service.publicState(created)).not.toHaveProperty('merchantWallet');
    const completed = await service.complete(created.id, {
      tx_hash: txHash, customer_wallet_address: subscriber, subscription_id: subscriptionId,
    });
    expect(completed.status).toBe('confirmed');
    expect(completed.txHash).toBe(txHash);
    expect(await service.complete(created.id, {
      tx_hash: txHash, customer_wallet_address: subscriber, subscription_id: subscriptionId,
    })).toEqual(completed);
    await expect(service.complete(created.id, {
      tx_hash: 'b'.repeat(64), customer_wallet_address: subscriber, subscription_id: subscriptionId,
    })).rejects.toThrow(/already been completed/i);
    await service.close();
  });

  it('rejects forged payment, mismatched wallet and unavailable on-chain approval', async () => {
    expect(matchingVaultEvent({ ...confirmed(), status: 'FAILED' }, {
      contractId: contract, tokenContractId: token,
      customerWallet: subscriber, merchantWallet: merchant,
      amount: plan.usdc_amount, intervalSeconds: plan.interval_seconds,
    })).toBe(false);
    expect(matchingVaultEvent(confirmed({ amount: 1n }), {
      contractId: contract, tokenContractId: token,
      customerWallet: subscriber, merchantWallet: merchant,
      amount: plan.usdc_amount, intervalSeconds: plan.interval_seconds,
    })).toBe(false);
    const { service } = makeService({ txn: { status: 'PENDING' } });
    const created = await service.create(planId);
    await expect(service.complete(created.id, {
      tx_hash: txHash, customer_wallet_address: subscriber, subscription_id: subscriptionId,
    })).rejects.toThrow(/matching confirmed on-chain allowance/i);
    await expect(service.complete(created.id, {
      tx_hash: txHash, customer_wallet_address: Keypair.random().publicKey(), subscription_id: subscriptionId,
    })).rejects.toThrow(/does not match/i);
    await service.close();
  });

  it('expires after five minutes even if a valid transaction confirms later', async () => {
    const { service, tick } = makeService();
    const created = await service.create(planId);
    tick(HANDOVER_TTL_MS + 1);
    expect(await service.load(created.id)).toBeNull();
    await expect(service.complete(created.id, {
      tx_hash: txHash, customer_wallet_address: subscriber, subscription_id: subscriptionId,
    })).rejects.toThrow(/expired/i);
    await service.close();
  });

  it('rejects an old transaction reused for a newly created session', async () => {
    const { service } = makeService({ txn: { ...confirmed(), ledger: 100 } });
    const created = await service.create(planId);
    await expect(service.complete(created.id, {
      tx_hash: txHash, customer_wallet_address: subscriber, subscription_id: subscriptionId,
    })).rejects.toThrow(/matching confirmed/i);
    await service.close();
  });
});
