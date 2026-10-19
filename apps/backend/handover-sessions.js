'use strict';

const crypto = require('node:crypto');
const { rpc, scValToNative, StrKey } = require('@stellar/stellar-sdk');

const HANDOVER_TTL_MS = 5 * 60 * 1000;
const SESSION_KEY = 'orbit:handover:';
const CHANNEL_PATTERN = /^[0-9a-f]{48}$/i;
const TX_HASH_PATTERN = /^[0-9a-f]{64}$/i;
const MAX_LOCAL_SESSIONS = 1024;

function matchingVaultEvent(confirmation, {
  contractId, customerWallet, merchantWallet, tokenContractId, amount, intervalSeconds,
}) {
  if (confirmation?.status !== 'SUCCESS') return false;
  const entries = confirmation?.events?.contractEventsXdr;
  if (!Array.isArray(entries)) return false;
  return entries.flat().some((event) => {
    try {
      if (StrKey.encodeContract(event.contractId()) !== contractId) return false;
      const topics = event.body().v0().topics().map(scValToNative);
      if (!['vault_created', 'trial_vault_created'].includes(topics[0]) ||
          topics[1] !== customerWallet || topics[2] !== merchantWallet) return false;
      const data = scValToNative(event.body().v0().data());
      return Array.isArray(data) && data.length >= 3 &&
        data[0] === tokenContractId &&
        BigInt(data[1]) === BigInt(amount) &&
        BigInt(data[2]) === BigInt(intervalSeconds);
    } catch {
      return false;
    }
  });
}

function createHandoverService({
  supabaseProvider, rpcUrl, contractId, tokenContractId, redisUrl = process.env.REDIS_URL,
  now = Date.now, serverFactory = (url) => new rpc.Server(url),
  RedisFactory,
} = {}) {
  const local = new Map();
  let redis = null;
  if (redisUrl) {
    const Redis = RedisFactory || require('ioredis');
    redis = new Redis(redisUrl, { maxRetriesPerRequest: 1, connectTimeout: 3000 });
    redis.on('error', (error) => console.error('Handover Redis connection failed:', error.message));
  }
  const keyFor = (id) => SESSION_KEY + id;
  const purge = () => {
    for (const [id, value] of local) if (now() >= value.expiresAt) local.delete(id);
  };
  async function load(id) {
    if (!CHANNEL_PATTERN.test(id)) return null;
    if (redis) {
      const raw = await redis.get(keyFor(id));
      return raw ? JSON.parse(raw) : null;
    }
    purge();
    return local.get(id) || null;
  }
  async function save(session) {
    if (redis) {
      await redis.set(keyFor(session.id), JSON.stringify(session), 'PX', Math.max(1000, session.expiresAt - now()));
    } else {
      purge();
      if (local.size >= MAX_LOCAL_SESSIONS && !local.has(session.id)) {
        throw new Error('Handover session capacity exceeded');
      }
      local.set(session.id, session);
    }
  }
  async function create(planId) {
    if (typeof planId !== 'string' || !/^[\w-]{8,100}$/.test(planId)) {
      const error = new Error('Invalid plan identifier'); error.status = 400; throw error;
    }
    if (!contractId || !tokenContractId) {
      const error = new Error('Handover is not configured on this deployment'); error.status = 503; throw error;
    }
    const supabase = supabaseProvider();
    if (!supabase) {
      const error = new Error('Payment database is unavailable'); error.status = 503; throw error;
    }
    const { data: plan, error: planError } = await supabase.from('plans')
      .select('id,merchant_id,usdc_amount,interval_seconds,merchants(wallet_address)')
      .eq('id', planId).maybeSingle();
    if (planError) throw planError;
    if (!plan || !plan.merchants?.wallet_address) {
      const error = new Error('Plan not found'); error.status = 404; throw error;
    }
    const server = serverFactory(rpcUrl);
    const latest = await server.getLatestLedger();
    const id = crypto.randomBytes(24).toString('hex');
    const session = {
      id, planId: plan.id, merchantWallet: plan.merchants.wallet_address,
      amount: String(plan.usdc_amount), interval: String(plan.interval_seconds),
      openedLedger: Number(latest.sequence || 0), status: 'pending',
      expiresAt: now() + HANDOVER_TTL_MS, txHash: null, customerWallet: null,
      subscriptionId: null,
    };
    await save(session);
    return session;
  }
  async function complete(id, { tx_hash: txHash, customer_wallet_address: wallet, subscription_id: subscriptionId }) {
    const session = await load(id);
    if (!session || now() >= session.expiresAt) {
      const error = new Error('Handover session expired or missing'); error.status = 410; throw error;
    }
    if (session.status === 'confirmed') {
      if (session.txHash === txHash && session.subscriptionId === subscriptionId) return session;
      const error = new Error('Session has already been completed'); error.status = 409; throw error;
    }
    if (!TX_HASH_PATTERN.test(txHash || '') || !StrKey.isValidEd25519PublicKey(wallet || '') ||
        typeof subscriptionId !== 'string' || subscriptionId.length > 100) {
      const error = new Error('A confirmed tx hash, wallet and subscription ID are required'); error.status = 400; throw error;
    }
    const supabase = supabaseProvider();
    if (!supabase) {
      const error = new Error('Payment database is unavailable'); error.status = 503; throw error;
    }
    const { data: subscription, error: dbError } = await supabase.from('subscriptions')
      .select('id,plan_id,customer_wallet_address')
      .eq('id', subscriptionId).maybeSingle();
    if (dbError) throw dbError;
    if (!subscription || subscription.plan_id !== session.planId ||
        subscription.customer_wallet_address !== wallet) {
      const error = new Error('This subscription does not match the handover session'); error.status = 403; throw error;
    }
    const confirmation = await serverFactory(rpcUrl).getTransaction(txHash);
    if (confirmation?.status !== 'SUCCESS' ||
        Number(confirmation.ledger || 0) < session.openedLedger ||
        !matchingVaultEvent(confirmation, {
          contractId, customerWallet: wallet, merchantWallet: session.merchantWallet,
          tokenContractId, amount: session.amount, intervalSeconds: session.interval,
        })) {
      const error = new Error('No matching confirmed on-chain allowance found'); error.status = 422; throw error;
    }
    if (now() >= session.expiresAt) {
      const error = new Error('Handover session expired'); error.status = 410; throw error;
    }
    const completed = { ...session, status: 'confirmed', txHash, subscriptionId, customerWallet: wallet };
    await save(completed);
    return completed;
  }
  function publicState(session) {
    return {
      id: session.id, planId: session.planId, status: session.status,
      expiresAt: session.expiresAt,
      confirmation: session.status === 'confirmed' ? {
        txHash: session.txHash, subscriptionId: session.subscriptionId,
        customerWalletAddress: session.customerWallet,
      } : null,
    };
  }
  async function close() { if (redis) await redis.quit(); local.clear(); }
  return { create, load, complete, publicState, close };
}

function registerHandoverRoutes(app, supabaseProvider, options = {}) {
  const service = createHandoverService({ supabaseProvider, ...options });
  const respondError = (res, error) => res.status(error.status || 503).json({ error: error.message });
  app.post('/handover/sessions', async (req, res) => {
    try {
      const session = await service.create(req.body?.plan_id);
      res.set('Cache-Control', 'no-store');
      res.status(201).json({ session: service.publicState(session) });
    } catch (error) { respondError(res, error); }
  });
  app.get('/handover/sessions/:id', async (req, res) => {
    try {
      const session = await service.load(req.params.id);
      res.set('Cache-Control', 'no-store');
      if (!session) return res.status(410).json({ session: { status: 'expired' } });
      res.json({ session: service.publicState(session) });
    } catch (error) { respondError(res, error); }
  });
  app.post('/handover/sessions/:id/complete', async (req, res) => {
    try {
      const session = await service.complete(req.params.id, req.body || {});
      res.set('Cache-Control', 'no-store');
      res.json({ session: service.publicState(session) });
    } catch (error) { respondError(res, error); }
  });
  app.locals.handoverService = service;
  return service;
}

module.exports = { createHandoverService, registerHandoverRoutes, matchingVaultEvent, HANDOVER_TTL_MS };
