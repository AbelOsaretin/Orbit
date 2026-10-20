import { describe, it, expect, vi } from 'vitest';
import {
  PREBILLING_WINDOW_HOURS,
  scanUpcomingRenewals,
  evaluateLiquidity,
  checkOnChainLiquidity,
  runPreBillingScan,
} from '../prebilling-monitor.js';
import { createFakeSupabase } from './fakeSupabase.js';

const MERCHANT_ID = '7f8b9a10-b2c3-4d5e-a6f7-112233445566';

const makeSubscription = (overrides = {}) => ({
  id: 'sub-1',
  plan_id: 'plan-1',
  customer_wallet_address: 'GWALLET',
  status: 'active',
  next_billing_date: new Date('2026-01-02T00:00:00Z').toISOString(),
  plans: {
    id: 'plan-1',
    merchant_id: MERCHANT_ID,
    name: 'Pro Plan',
    usdc_amount: '290000000',
    interval_seconds: 2592000,
  },
  ...overrides,
});

const makeChain = (balance, allowance) => ({
  tokenAddress: 'CTOKEN',
  spender: 'CCONTRACT',
  getTokenBalance: vi.fn(async () => balance),
  getAllowance: vi.fn(async () => allowance),
});

const makeFetch = () => vi.fn(async () => ({ ok: true, status: 200, text: async () => '{"received":true}' }));

const seedDb = (subscriptions) =>
  createFakeSupabase({
    subscriptions,
    webhooks: [
      {
        id: 'wh-1',
        merchant_id: MERCHANT_ID,
        url: 'https://merchant.example/hooks',
        events: ['*'],
        active: true,
        signing_secret: 'whsec_test',
      },
    ],
    webhook_deliveries: [],
  });

describe('Pre-billing liquidity monitor (#208)', () => {
  it('defines the 72-hour pre-billing scan window', () => {
    expect(PREBILLING_WINDOW_HOURS).toBe(72);
  });

  it('scans only active subscriptions renewing within the window', async () => {
    const now = new Date('2026-01-01T00:00:00Z');
    const inWindow = makeSubscription({ id: 'in-window' });
    const outsideWindow = makeSubscription({
      id: 'outside',
      next_billing_date: new Date('2026-01-05T00:00:00Z').toISOString(),
    });
    const cancelled = makeSubscription({
      id: 'cancelled',
      status: 'cancelled',
    });
    const db = seedDb([inWindow, outsideWindow, cancelled]);

    const { renewals } = await scanUpcomingRenewals(db, { now });

    expect(renewals.map((r) => r.id)).toEqual(['in-window']);
  });

  it('classifies balance and allowance shortfalls against the required amount', () => {
    expect(
      evaluateLiquidity({ balance: 100n, allowance: 100n, requiredAmount: 50n }),
    ).toMatchObject({ lowBalance: false, allowanceExpiring: false });

    expect(
      evaluateLiquidity({ balance: 10n, allowance: 100n, requiredAmount: 50n }),
    ).toMatchObject({ lowBalance: true, allowanceExpiring: false });

    expect(
      evaluateLiquidity({ balance: 100n, allowance: 10n, requiredAmount: 50n }),
    ).toMatchObject({ lowBalance: false, allowanceExpiring: true });
  });

  it('pre-flights on-chain balance and allowance through the chain client', async () => {
    const chain = makeChain('10', '500');
    const risk = await checkOnChainLiquidity(chain, {
      subscriber: 'GWALLET',
      tokenAddress: 'CTOKEN',
      spender: 'CCONTRACT',
      requiredAmount: '100',
    });

    expect(chain.getTokenBalance).toHaveBeenCalledWith({ tokenAddress: 'CTOKEN', account: 'GWALLET' });
    expect(chain.getAllowance).toHaveBeenCalledWith({ tokenAddress: 'CTOKEN', owner: 'GWALLET', spender: 'CCONTRACT' });
    expect(risk.lowBalance).toBe(true);
    expect(risk.allowanceExpiring).toBe(false);
  });

  it('dispatches subscription.low_balance_warning before a failing pull', async () => {
    const db = seedDb([makeSubscription()]);
    const chain = makeChain('0', '290000000');
    const fetchImpl = makeFetch();

    const result = await runPreBillingScan(db, {
      now: new Date('2026-01-01T00:00:00Z'),
      chain,
      fetchImpl,
    });

    expect(result.processed).toBe(1);
    expect(result.warnings).toHaveLength(1);
    expect(result.warnings[0]).toMatchObject({
      event: 'subscription.low_balance_warning',
      subscription_id: 'sub-1',
      customer_wallet_address: 'GWALLET',
    });
    expect(result.warnings[0].topup_url).toContain('action=topup');
    expect(fetchImpl).toHaveBeenCalledWith(
      'https://merchant.example/hooks',
      expect.objectContaining({
        headers: expect.objectContaining({ 'X-Orbit-Event': 'subscription.low_balance_warning' }),
      }),
    );
  });

  it('dispatches subscription.allowance_expiring when the remaining allowance is short', async () => {
    const db = seedDb([makeSubscription()]);
    const chain = makeChain('290000000', '0');
    const fetchImpl = makeFetch();

    const { warnings } = await runPreBillingScan(db, {
      now: new Date('2026-01-01T00:00:00Z'),
      chain,
      fetchImpl,
    });

    expect(warnings).toHaveLength(1);
    expect(warnings[0].event).toBe('subscription.allowance_expiring');
    expect(warnings[0].reapprove_url).toContain('action=reapprove');
    expect(fetchImpl).toHaveBeenCalledWith(
      'https://merchant.example/hooks',
      expect.objectContaining({
        headers: expect.objectContaining({ 'X-Orbit-Event': 'subscription.allowance_expiring' }),
      }),
    );
  });

  it('dispatches both warnings when balance and allowance are short', async () => {
    const db = seedDb([makeSubscription()]);
    const chain = makeChain('1', '1');
    const fetchImpl = makeFetch();

    const { warnings } = await runPreBillingScan(db, {
      now: new Date('2026-01-01T00:00:00Z'),
      chain,
      fetchImpl,
    });

    expect(warnings.map((w) => w.event)).toEqual([
      'subscription.low_balance_warning',
      'subscription.allowance_expiring',
    ]);
    expect(fetchImpl).toHaveBeenCalledTimes(2);
  });

  it('dispatches nothing when the subscriber can cover the renewal', async () => {
    const db = seedDb([makeSubscription()]);
    const chain = makeChain('290000000', '290000000');
    const fetchImpl = makeFetch();

    const result = await runPreBillingScan(db, {
      now: new Date('2026-01-01T00:00:00Z'),
      chain,
      fetchImpl,
    });

    expect(result.processed).toBe(1);
    expect(result.warnings).toEqual([]);
    expect(fetchImpl).not.toHaveBeenCalled();
  });
});
