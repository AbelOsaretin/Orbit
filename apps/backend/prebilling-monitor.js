/**
 * Issue #208: Predictive pre-billing balance and allowance monitor.
 * Scans subscriptions renewing within 72 hours, simulates the on-chain
 * balance/allowance pre-flight, and dispatches low-balance and
 * allowance-expiring warnings before the settlement pull can fail.
 */

const { dispatchWebhookEvent } = require('./webhooks');
const { generateRecoveryLink } = require('./dunning');

const PREBILLING_WINDOW_HOURS = 72;

/** Renewals settling within `windowHours` from `now`, still active. */
const scanUpcomingRenewals = async (supabase, { now = new Date(), windowHours = PREBILLING_WINDOW_HOURS } = {}) => {
  if (!supabase) return { renewals: [], error: 'supabase not initialized' };
  const windowEnd = new Date(now.getTime() + windowHours * 60 * 60 * 1000);
  const { data, error } = await supabase
    .from('subscriptions')
    .select('*, plans(*)')
    .eq('status', 'active')
    .gte('next_billing_date', now.toISOString())
    .lte('next_billing_date', windowEnd.toISOString());
  if (error) return { renewals: [], error: error.message };
  return { renewals: data || [] };
};

/** Pure shortfall classification used by the scan and its tests. */
const evaluateLiquidity = ({ balance, allowance, requiredAmount }) => {
  const required = BigInt(requiredAmount);
  return {
    requiredAmount: required.toString(),
    balance: BigInt(balance).toString(),
    allowance: BigInt(allowance).toString(),
    lowBalance: BigInt(balance) < required,
    allowanceExpiring: BigInt(allowance) < required,
  };
};

/**
 * Pre-flight on-chain check for one renewal: current token balance and the
 * allowance the billing contract can still pull.
 */
const checkOnChainLiquidity = async (chain, { subscriber, tokenAddress, spender, requiredAmount }) => {
  const balance = await chain.getTokenBalance({ tokenAddress, account: subscriber });
  const allowance = await chain.getAllowance({ tokenAddress, owner: subscriber, spender });
  return evaluateLiquidity({ balance, allowance, requiredAmount });
};

const warningPayload = ({ sub, risk, link, merchantId }) => {
  const event = risk.allowanceExpiring && !risk.lowBalance ? 'subscription.allowance_expiring' : 'subscription.low_balance_warning';
  return {
    merchantId,
    eventType: event,
    payload: {
      event,
      subscription_id: sub.id,
      customer_wallet_address: sub.customer_wallet_address,
      plan_id: sub.plan_id,
      required_amount: risk.requiredAmount,
      balance: risk.balance,
      allowance: risk.allowance,
      next_billing_date: sub.next_billing_date,
      topup_url: link.topup_url,
      reapprove_url: link.reapprove_url,
    },
  };
};

const buildLinks = (sub, options = {}) => {
  const recovery = generateRecoveryLink(sub.id, { planId: sub.plan_id, baseUrl: options.baseUrl });
  return {
    topup_url: `${recovery.recovery_url}&action=topup`,
    reapprove_url: `${recovery.recovery_url}&action=reapprove`,
  };
};

/**
 * Runs one pre-billing scan tick: every renewal inside the window is
 * pre-flighted and any shortfall dispatches webhook warnings.
 */
const runPreBillingScan = async (supabase, {
  now = new Date(),
  windowHours = PREBILLING_WINDOW_HOURS,
  chain,
  fetchImpl = globalThis.fetch,
  baseUrl,
} = {}) => {
  if (!chain) return { processed: 0, warnings: [], error: 'chain client not configured' };
  const { renewals, error } = await scanUpcomingRenewals(supabase, { now, windowHours });
  if (error) return { processed: 0, warnings: [], error };

  const warnings = [];
  for (const sub of renewals) {
    const requiredAmount = sub.plans?.usdc_amount ?? 0;
    const merchantId = sub.plans?.merchant_id;
    const risk = await checkOnChainLiquidity(chain, {
      subscriber: sub.customer_wallet_address,
      tokenAddress: chain.tokenAddress,
      spender: chain.spender,
      requiredAmount,
    });

    if (!risk.lowBalance && !risk.allowanceExpiring) continue;

    const link = buildLinks(sub, { baseUrl });
    const events = [];
    if (risk.lowBalance) {
      events.push(warningPayload({ sub, risk: { ...risk, allowanceExpiring: false }, link, merchantId }));
    }
    if (risk.allowanceExpiring) {
      events.push(warningPayload({ sub, risk: { ...risk, lowBalance: false }, link, merchantId }));
    }

    for (const event of events) {
      await dispatchWebhookEvent(supabase, { ...event, fetchImpl });
      warnings.push({ subscription_id: sub.id, ...event.payload });
    }
  }

  return { processed: renewals.length, warnings };
};

/** Schedules the scan worker; returns a stop handle. */
const startPreBillingMonitor = ({ supabase, chain, intervalMs = 60 * 60 * 1000, ...rest } = {}) => {
  const timer = setInterval(() => {
    runPreBillingScan(supabase, { chain, ...rest }).catch((err) => {
      console.error('Pre-billing monitor scan failed:', err);
    });
  }, intervalMs);
  if (typeof timer.unref === 'function') timer.unref();
  return { stop: () => clearInterval(timer) };
};

module.exports = {
  PREBILLING_WINDOW_HOURS,
  scanUpcomingRenewals,
  evaluateLiquidity,
  checkOnChainLiquidity,
  runPreBillingScan,
  startPreBillingMonitor,
};
