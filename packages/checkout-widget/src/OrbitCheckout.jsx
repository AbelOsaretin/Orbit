import React, { useEffect, useMemo, useRef, useState } from 'react';
import gsap from 'gsap';
import { completeHandover, openHandover, readHandover, watchHandover } from './mobileHandover';
import { StellarWalletsKit } from '@creit.tech/stellar-wallets-kit/sdk';
import { FreighterModule, FREIGHTER_ID } from '@creit.tech/stellar-wallets-kit/modules/freighter';
import { AlbedoModule, ALBEDO_ID } from '@creit.tech/stellar-wallets-kit/modules/albedo';
import { LobstrModule, LOBSTR_ID } from '@creit.tech/stellar-wallets-kit/modules/lobstr';
import { HanaModule, HANA_ID } from '@creit.tech/stellar-wallets-kit/modules/hana';
import { xBullModule, XBULL_ID } from '@creit.tech/stellar-wallets-kit/modules/xbull';
import { Networks, rpc } from '@stellar/stellar-sdk';
import { getNetworkDetails, requestAccess } from '@stellar/freighter-api';
import { authorizePlan, authorizePlanAtomic, PendingTransactionError, pollTransaction, preflightPlan } from './soroban';
import { assetContractId, assetKey, authorizeSwapPlan, fetchPayWithBalances, fetchSwapQuote, fromStroops, horizonUrlFor, payWithAssetsFor } from './swap';
import { isPasskeySupported, authenticateOrRegisterPasskey, PasskeyWalletAdapter } from './passkey';
import {
  isSessionKeySupported,
  createDelegatedSessionKey,
  getActiveSessionKey,
  revokeSessionKey,
  SessionWalletAdapter,
  SMART_WALLET_ID,
} from './sessionKey';

const STORAGE_KEY = 'orbit:selected-wallet';
const PENDING_KEY = 'orbit:pending-transaction';
const PASSKEY_WALLET_ID = 'orbit:passkey';
const WALLET_MODULES = [
  [FREIGHTER_ID, 'Freighter', FreighterModule],
  [ALBEDO_ID, 'Albedo', AlbedoModule],
  [LOBSTR_ID, 'Lobstr', LobstrModule],
  [HANA_ID, 'Hana', HanaModule],
  [XBULL_ID, 'xBull', xBullModule],
];
const DEFAULT_WALLETS = WALLET_MODULES.map(([id]) => id);
const DEMO_ADDRESS = 'GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5';
const PLAN_CURRENCY_CODE = 'USDC';
const SLIPPAGE_OPTIONS = [0.5, 1, 3];
const HIGH_PRICE_IMPACT_PCT = 5;
const RADIUS = { none: '0', sm: '4px', md: '10px', lg: '18px', full: '999px' };

export default function OrbitCheckout({
  planId,
  planData,
  apiUrl = 'http://localhost:3001',
  demo = false,
  allowedWallets = DEFAULT_WALLETS,
  orbitContractId = import.meta.env.VITE_ORBIT_CONTRACT_ID,
  tokenContractId = import.meta.env.VITE_USDC_CONTRACT_ID,
  rpcUrl = import.meta.env.VITE_SOROBAN_RPC_URL || 'https://soroban-testnet.stellar.org',
  networkPassphrase = import.meta.env.VITE_STELLAR_NETWORK_PASSPHRASE || Networks.TESTNET,
  theme = 'auto',
  accentColor = '#6366f1',
  borderRadius = 'md',
  fontFamily = 'inherit',
  customClasses = {},
  allowedOrigins = [],
  customFields = [],
}) {
  const walletChoices = useMemo(() => WALLET_MODULES.filter(([id]) => allowedWallets.includes(id)), [allowedWallets]);
  const [plan, setPlan] = useState(planData || null);
  const [loading, setLoading] = useState(!planData);
  const [error, setError] = useState('');
  const [address, setAddress] = useState('');
  const [showWallets, setShowWallets] = useState(false);
  const [processing, setProcessing] = useState(false);
  const [receipt, setReceipt] = useState(null);
  const [handoverSession, setHandoverSession] = useState(null);
  const [handoverBusy, setHandoverBusy] = useState(false);
  const handoverWatcher = useRef(null);
  const successElement = useRef(null);
  const incomingChannelId = useMemo(() => {
    if (typeof window === 'undefined') return null;
    return new URLSearchParams(window.location.search).get('orbit_handover');
  }, []);
  const [isDemo, setIsDemo] = useState(false);
  const [feeEstimate, setFeeEstimate] = useState(null);
  const [pending, setPending] = useState(null);
  const [confirmingSeconds, setConfirmingSeconds] = useState(60);
  const [confirming, setConfirming] = useState(false);

  useEffect(() => () => handoverWatcher.current?.stop(), []);

  useEffect(() => {
    if (receipt && successElement.current) {
      const animation = gsap.fromTo(successElement.current,
        { autoAlpha: 0, scale: 0.94, y: 14 },
        { autoAlpha: 1, scale: 1, y: 0, duration: 0.55, ease: 'power2.out' });
      return () => animation.kill();
    }
    return undefined;
  }, [receipt]);

  // Multi-asset payment (#191): balances, selected pay-with asset, and live swap quote
  const [payWithBalances, setPayWithBalances] = useState([]);
  const [payWithKey, setPayWithKey] = useState('');
  const [slippagePct, setSlippagePct] = useState(1);
  const [swapQuote, setSwapQuote] = useState(null);
  const [swapQuoteError, setSwapQuoteError] = useState('');
  const [swapQuoteLoading, setSwapQuoteLoading] = useState(false);

  // Custom Form Fields & Validation
  const [formData, setFormData] = useState({});
  const [formErrors, setFormErrors] = useState({});

  // Network Detection & Auto-switching
  const [isWrongNetwork, setIsWrongNetwork] = useState(false);
  const [detectedNetwork, setDetectedNetwork] = useState('');
  const [switchingNetwork, setSwitchingNetwork] = useState(false);

  // Passkey Smart Wallet
  const [passkeyAdapter, setPasskeyAdapter] = useState(null);
  const hasPasskey = useMemo(() => isPasskeySupported(), []);

  // Smart Wallet & Session Key Delegation (#169)
  const [isSessionKey, setIsSessionKey] = useState(false);
  const [sessionData, setSessionData] = useState(null);
  const hasSessionKey = useMemo(() => isSessionKeySupported(), []);

  useEffect(() => {
    const saved = localStorage.getItem(STORAGE_KEY);
    if (saved === SMART_WALLET_ID) {
      const activeSession = getActiveSessionKey();
      if (activeSession) {
        const adapter = new SessionWalletAdapter(activeSession);
        setPasskeyAdapter(adapter);
        setSessionData(activeSession);
        setIsSessionKey(true);
        setAddress(activeSession.address);
      }
    }
  }, []);

  const actualTheme = theme === 'auto' && typeof window !== 'undefined' && window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : theme;
  const rootStyle = {
    '--orbit-accent': accentColor,
    '--orbit-radius': RADIUS[borderRadius] || RADIUS.md,
    '--orbit-font': fontFamily,
    '--orbit-surface': actualTheme === 'dark' ? '#171923' : '#fff',
    '--orbit-text': actualTheme === 'dark' ? '#f8fafc' : '#101828',
    '--orbit-muted': actualTheme === 'dark' ? '#a7b0c0' : '#667085',
    '--orbit-border': actualTheme === 'dark' ? '#34394a' : '#d0d5dd',
    '--orbit-error': actualTheme === 'dark' ? '#ffb4ab' : '#b42318',
  };

  const dispatch = (type, detail = {}) => {
    if (typeof window === 'undefined' || window.parent === window) return;
    const origins = allowedOrigins.length ? allowedOrigins : [window.location.origin];
    origins.forEach((origin) => window.parent.postMessage({ type, ...detail }, origin));
  };

  const validateForm = () => {
    if (!customFields || customFields.length === 0) return true;
    const errors = {};
    for (const field of customFields) {
      const val = (formData[field.id] || '').trim();
      if (field.required && !val) {
        errors[field.id] = `${field.label || field.id} is required.`;
      } else if (field.type === 'email' && val) {
        if (!/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(val)) {
          errors[field.id] = 'Please enter a valid email address.';
        }
      }
    }
    setFormErrors(errors);
    return Object.keys(errors).length === 0;
  };

  const handleFieldChange = (id, val) => {
    setFormData((prev) => ({ ...prev, [id]: val }));
    if (formErrors[id]) {
      setFormErrors((prev) => {
        const next = { ...prev };
        delete next[id];
        return next;
      });
    }
  };

  const checkNetwork = async () => {
    try {
      const details = await getNetworkDetails();
      if (details?.networkPassphrase) {
        setDetectedNetwork(details.networkPassphrase);
        const mismatch = details.networkPassphrase !== networkPassphrase;
        setIsWrongNetwork(mismatch);
        return !mismatch;
      }
    } catch {
      // Wallet extension not active or network details unavailable
    }
    setIsWrongNetwork(false);
    return true;
  };

  const handleSwitchNetwork = async () => {
    setSwitchingNetwork(true);
    setError('');
    try {
      const targetName = networkPassphrase === Networks.PUBLIC ? 'Mainnet' : 'Testnet';
      if (typeof window !== 'undefined' && (window.freighterApi?.requestAccess || requestAccess)) {
        await requestAccess();
        const ok = await checkNetwork();
        if (!ok) {
          setError(`Please confirm network switch to ${targetName} inside your wallet.`);
        }
      } else {
        setError(`Please switch your wallet network to ${targetName} directly in your wallet.`);
      }
    } catch (err) {
      setError(err?.message || 'Could not trigger network switch.');
    } finally {
      setSwitchingNetwork(false);
    }
  };

  useEffect(() => {
    StellarWalletsKit.init({ modules: walletChoices.map(([, , Module]) => new Module()) });
    const saved = localStorage.getItem(STORAGE_KEY);
    if (saved === PASSKEY_WALLET_ID) {
      // Attempt auto-reconnect passkey
      authenticateOrRegisterPasskey()
        .then(({ address: value, keypair }) => {
          setAddress(value);
          setPasskeyAdapter(new PasskeyWalletAdapter(keypair));
          dispatch('orbit:wallet_connected', { address: value, type: 'passkey' });
        })
        .catch(() => {
          localStorage.removeItem(STORAGE_KEY);
        });
    } else if (saved && walletChoices.some(([id]) => id === saved)) {
      StellarWalletsKit.setWallet(saved);
      StellarWalletsKit.getAddress()
        .then(({ address: value }) => {
          setAddress(value);
          dispatch('orbit:wallet_connected', { address: value });
          checkNetwork();
        })
        .catch(() => {});
    }
  }, [walletChoices]);

  useEffect(() => {
    if (planData || !planId) return undefined;
    const controller = new AbortController();
    fetch(`${apiUrl}/plans/${planId}`, { signal: controller.signal })
      .then(async (response) => {
        if (!response.ok) throw new Error(response.status === 404 ? 'Plan not found.' : 'Unable to load this plan.');
        return response.json();
      })
      .then(({ plan: value }) => setPlan(value))
      .catch((reason) => reason.name !== 'AbortError' && setError(reason.message))
      .finally(() => setLoading(false));
    return () => controller.abort();
  }, [apiUrl, planData, planId]);

  useEffect(() => {
    if (plan) dispatch('orbit:ready', { planId: plan.id || planId });
  }, [plan, planId]);

  // Scanning the QR opens the same merchant checkout page inside the mobile
  // wallet's DApp browser. The server-resolved plan overrides desktop defaults.
  useEffect(() => {
    if (!incomingChannelId) return undefined;
    let mounted = true;
    const controller = new AbortController();
    (async () => {
      const session = await readHandover({ apiUrl, channelId: incomingChannelId });
      if (session.status !== 'pending') throw new Error('Mobile approval session is no longer active.');
      const response = await fetch(`${apiUrl}/plans/${encodeURIComponent(session.planId)}`, { signal: controller.signal });
      if (!response.ok) throw new Error('Unable to load the mobile approval plan.');
      const { plan: remotePlan } = await response.json();
      if (!remotePlan || remotePlan.id !== session.planId) throw new Error('Mobile checkout plan mismatch.');
      if (mounted) { setPlan(remotePlan); setLoading(false); }
    })().catch((reason) => {
      if (mounted && reason.name !== 'AbortError') { setError(reason.message); setLoading(false); }
    });
    return () => { mounted = false; controller.abort(); };
  }, [incomingChannelId, apiUrl]);

  useEffect(() => {
    const saved = localStorage.getItem(PENDING_KEY);
    if (!saved) return undefined;
    let transaction;
    try {
      transaction = JSON.parse(saved);
    } catch {
      localStorage.removeItem(PENDING_KEY);
      return undefined;
    }
    setPending(transaction);
    pollTransaction(new rpc.Server(rpcUrl), transaction.hash, {
      onProgress: ({ remainingMs }) => setConfirmingSeconds(Math.ceil(remainingMs / 1000)),
    })
      .then(() => {
        localStorage.removeItem(PENDING_KEY);
        setPending(null);
        if (transaction.stage === 'approval') {
          setError('USDC authorization confirmed. Reconnect your wallet and complete the vault subscription.');
        } else {
          setReceipt({ vaultHash: transaction.hash, recovered: true });
        }
      })
      .catch((reason) => {
        if (!(reason instanceof PendingTransactionError)) {
          localStorage.removeItem(PENDING_KEY);
          setPending(null);
        }
      });
    return undefined;
  }, [rpcUrl]);

  const handleOpenWallets = () => {
    if (!validateForm()) return;
    setShowWallets(true);
  };

  const connect = async (walletId) => {
    try {
      setError('');
      setPasskeyAdapter(null);
      StellarWalletsKit.setWallet(walletId);
      const result = await StellarWalletsKit.getAddress();
      localStorage.setItem(STORAGE_KEY, walletId);
      setAddress(result.address);
      setIsDemo(false);
      setReceipt(null);
      setShowWallets(false);
      dispatch('orbit:wallet_connected', { address: result.address });
      await checkNetwork();
    } catch (reason) {
      const message = reason?.message || 'Wallet connection was cancelled.';
      setError(message);
      dispatch('orbit:error', { message });
    }
  };

  const connectPasskey = async () => {
    try {
      setError('');
      const { address: passkeyAddress, keypair, isNew } = await authenticateOrRegisterPasskey({
        userName: formData.email || formData.name || plan?.name || 'Orbit Customer',
      });
      const adapter = new PasskeyWalletAdapter(keypair);
      setPasskeyAdapter(adapter);
      setIsSessionKey(false);
      setAddress(passkeyAddress);
      setIsDemo(false);
      setReceipt(null);
      setShowWallets(false);
      setIsWrongNetwork(false);
      localStorage.setItem(STORAGE_KEY, PASSKEY_WALLET_ID);
      dispatch('orbit:wallet_connected', { address: passkeyAddress, type: 'passkey', isNew });
    } catch (reason) {
      const message = reason?.message || 'Biometric passkey authentication failed.';
      setError(message);
      dispatch('orbit:error', { message });
    }
  };

  const connectSessionKey = async () => {
    try {
      setError('');
      const session = createDelegatedSessionKey({
        delegatedBy: formData.email || formData.name || 'Orbit Customer',
      });
      const adapter = new SessionWalletAdapter(session);
      setPasskeyAdapter(adapter);
      setSessionData(session);
      setIsSessionKey(true);
      setAddress(session.address);
      setIsDemo(false);
      setReceipt(null);
      setShowWallets(false);
      setIsWrongNetwork(false);
      localStorage.setItem(STORAGE_KEY, SMART_WALLET_ID);
      dispatch('orbit:wallet_connected', { address: session.address, type: 'smart-wallet-session', isSessionKey: true });
    } catch (reason) {
      const message = reason?.message || 'Failed to initialize smart wallet session.';
      setError(message);
      dispatch('orbit:error', { message });
    }
  };

  const disconnect = async () => {
    if (!isDemo && !passkeyAdapter) await StellarWalletsKit.disconnect().catch(() => {});
    if (isSessionKey) revokeSessionKey();
    localStorage.removeItem(STORAGE_KEY);
    setAddress('');
    setPasskeyAdapter(null);
    setSessionData(null);
    setIsSessionKey(false);
    setIsDemo(false);
    setReceipt(null);
    setIsWrongNetwork(false);
    dispatch('orbit:close');
  };

  const payWithAssets = useMemo(() => payWithAssetsFor(networkPassphrase), [networkPassphrase]);
  const horizonUrl = useMemo(() => horizonUrlFor(networkPassphrase), [networkPassphrase]);
  const planCurrency = useMemo(() => payWithAssets.find((asset) => asset.code === PLAN_CURRENCY_CODE), [payWithAssets]);
  const selectedPayWith = payWithBalances.find((asset) => assetKey(asset) === payWithKey)
    || payWithBalances.find((asset) => asset.code === PLAN_CURRENCY_CODE)
    || payWithBalances[0]
    || null;
  const isSwap = Boolean(selectedPayWith) && selectedPayWith.code !== PLAN_CURRENCY_CODE;

  // Load the wallet's spendable balances once it connects
  useEffect(() => {
    if (!address || isDemo) {
      setPayWithBalances([]);
      return undefined;
    }
    let cancelled = false;
    fetchPayWithBalances({ horizonUrl, address, assets: payWithAssets })
      .then((balances) => { if (!cancelled) setPayWithBalances(balances); })
      .catch(() => { if (!cancelled) setPayWithBalances([]); });
    return () => { cancelled = true; };
  }, [address, horizonUrl, payWithAssets, isDemo]);

  // Re-quote when the pay-with asset, slippage, or plan changes (debounced)
  useEffect(() => {
    if (!isSwap || !plan || !planCurrency) {
      setSwapQuote(null);
      setSwapQuoteError('');
      setSwapQuoteLoading(false);
      return undefined;
    }
    let cancelled = false;
    const timer = setTimeout(async () => {
      setSwapQuoteLoading(true);
      setSwapQuoteError('');
      try {
        const quote = await fetchSwapQuote({
          horizonUrl,
          sourceAsset: selectedPayWith,
          destAsset: planCurrency,
          destAmount: fromStroops(plan.usdc_amount),
          slippagePct,
        });
        if (cancelled) return;
        setSwapQuote(quote);
        if (!quote) setSwapQuoteError(`No swap path is available from ${selectedPayWith.code} right now.`);
      } catch (reason) {
        if (!cancelled) {
          setSwapQuote(null);
          setSwapQuoteError(reason.message || 'Could not load a swap quote.');
        }
      } finally {
        if (!cancelled) setSwapQuoteLoading(false);
      }
    }, 400);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [isSwap, selectedPayWith, slippagePct, plan, planCurrency, horizonUrl]);

  const startHandover = async () => {
    if (!plan?.id || !apiUrl) { setError('A live plan and payment API are required for mobile handover.'); return; }
    if (typeof window === 'undefined') return;
    setError(''); setHandoverBusy(true);
    handoverWatcher.current?.stop();
    try {
      const session = await openHandover({
        apiUrl, planId: plan.id, pageUrl: window.location.href,
      });
      setHandoverSession(session);
      setShowWallets(false);
      handoverWatcher.current = watchHandover({
        apiUrl, session,
        onConfirmed: (confirmed) => {
          setHandoverSession(null);
          setAddress(confirmed.customerWalletAddress || '');
          setReceipt({ vaultHash: confirmed.txHash, mobile: true });
          dispatch('orbit:subscribed', {
            subscriptionId: confirmed.subscriptionId, planId: session.planId, txHash: confirmed.txHash,
          });
        },
        onExpired: () => {
          setHandoverSession(null);
          setError('Mobile handover expired after five minutes. Start a new session.');
        },
        onError: (error) => {
          // Brief polling interruptions do not invalidate a valid on-chain payment.
          console.warn('Handover polling unavailable:', error.message);
        },
      });
    } catch (reason) {
      setError(reason.message || 'Could not start secure mobile handover.');
    } finally { setHandoverBusy(false); }
  };

  const cancelHandover = () => {
    handoverWatcher.current?.stop();
    handoverWatcher.current = null;
    setHandoverSession(null);
  };

  const subscribe = async () => {
    if (!validateForm()) return;
    if (isWrongNetwork) {
      setError('Please switch to the matching network before subscribing.');
      return;
    }

    setProcessing(true);
    setError('');
    try {
      if (isDemo) {
        setReceipt({ demo: true });
        return;
      }

      if (isSwap && (!swapQuote || swapQuoteLoading)) {
        throw new Error('Waiting for a live swap quote. Try again in a moment.');
      }
      if (isSwap && !plan.swap_router_address) {
        throw new Error(`${plan.merchants?.name || 'This merchant'} does not accept ${selectedPayWith.code} yet. Choose ${PLAN_CURRENCY_CODE} to continue.`);
      }

      const activeWallet = passkeyAdapter || StellarWalletsKit;
      setConfirming(true);
      setConfirmingSeconds(60);

      let hashes;
      if (isSwap) {
        hashes = {
          approvalHash: null,
          vaultHash: await authorizeSwapPlan({
            wallet: activeWallet, address,
            merchantAddress: plan.merchants.wallet_address,
            amount: plan.usdc_amount, intervalSeconds: plan.interval_seconds,
            orbitContractId, settlementTokenContractId: tokenContractId, rpcUrl, networkPassphrase,
            billingCycles: 12,
            paymentTokenContractId: assetContractId(selectedPayWith, networkPassphrase),
            routerAddress: plan.swap_router_address,
            maxInputStroops: swapQuote.maxInputStroops,
            onSubmitted: ({ stage, hash }) => {
              localStorage.setItem(PENDING_KEY, JSON.stringify({ stage, hash, planId: plan.id || planId }));
              setPending({ stage, hash });
            },
            onProgress: ({ remainingMs }) => setConfirmingSeconds(Math.ceil(remainingMs / 1000)),
          }),
        };
      } else if (isSessionKey || passkeyAdapter) {
        // Smart wallet sessions use a single atomic, sponsor-eligible handshake.
        hashes = await authorizePlanAtomic({
          wallet: activeWallet, address,
          merchantAddress: plan.merchants.wallet_address,
          amount: plan.usdc_amount, intervalSeconds: plan.interval_seconds,
          orbitContractId, tokenContractId, rpcUrl, networkPassphrase,
          apiUrl, sponsorFees: true, includeTrustline: true,
          onSubmitted: ({ stage, hash, sponsored }) => {
            localStorage.setItem(PENDING_KEY, JSON.stringify({ stage, hash, planId: plan.id || planId, sponsored }));
            setPending({ stage, hash, sponsored });
          },
          onProgress: ({ remainingMs }) => setConfirmingSeconds(Math.ceil(remainingMs / 1000)),
        });
      } else {
        const preflight = await preflightPlan({
          address, merchantAddress: plan.merchants.wallet_address,
          amount: plan.usdc_amount, intervalSeconds: plan.interval_seconds,
          orbitContractId, tokenContractId, rpcUrl, networkPassphrase,
        });
        setFeeEstimate(preflight);
        hashes = await authorizePlan({
          wallet: activeWallet, address,
          merchantAddress: plan.merchants.wallet_address,
          amount: plan.usdc_amount, intervalSeconds: plan.interval_seconds,
          orbitContractId, tokenContractId, rpcUrl, networkPassphrase,
          preflight,
          onSubmitted: ({ stage, hash }) => {
            localStorage.setItem(PENDING_KEY, JSON.stringify({ stage, hash, planId: plan.id || planId }));
            setPending({ stage, hash });
          },
          onProgress: ({ remainingMs }) => setConfirmingSeconds(Math.ceil(remainingMs / 1000)),
        });
      }

      const response = await fetch(`${apiUrl}/subscriptions`, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({
          plan_id: plan.id || planId,
          customer_wallet_address: address,
          metadata: formData,
        }),
      });

      if (!response.ok) throw new Error((await response.json()).error || 'Could not record the subscription.');
      const payload = await response.json();
      const subscription = payload.subscription || payload;
      if (incomingChannelId && !isDemo) {
        if (!subscription.id || !hashes.vaultHash) {
          throw new Error('The subscription was created but its payment proof is incomplete; keep the vault transaction hash.');
        }
        await completeHandover({
          apiUrl, channelId: incomingChannelId,
          txHash: hashes.vaultHash, subscriptionId: subscription.id,
          customerWalletAddress: address,
        });
      }
      localStorage.removeItem(PENDING_KEY);
      setPending(null);
      setReceipt(hashes);
      dispatch('orbit:subscribed', {
        subscriptionId: subscription.id,
        planId: plan.id || planId,
        txHash: hashes.vaultHash,
        metadata: formData,
      });
    } catch (reason) {
      if (reason instanceof PendingTransactionError) {
        setPending({ stage: 'transaction', hash: reason.hash });
      } else {
        const message = reason?.message || 'Authorization failed. No subscription was recorded.';
        setError(message);
        dispatch('orbit:error', { message });
      }
    } finally {
      setConfirming(false);
      setProcessing(false);
    }
  };

  const checkPending = async () => {
    if (!pending) return;
    setProcessing(true);
    setError('');
    setConfirming(true);
    try {
      await pollTransaction(new rpc.Server(rpcUrl), pending.hash, {
        onProgress: ({ remainingMs }) => setConfirmingSeconds(Math.ceil(remainingMs / 1000)),
      });
      localStorage.removeItem(PENDING_KEY);
      setPending(null);
      if (pending.stage === 'approval') {
        setError('USDC authorization confirmed. Reconnect your wallet and complete the vault subscription.');
      } else {
        setReceipt({ vaultHash: pending.hash, recovered: true });
      }
    } catch (reason) {
      if (!(reason instanceof PendingTransactionError)) setError(reason.message);
    } finally {
      setConfirming(false);
      setProcessing(false);
    }
  };

  if (loading) return <div className={customClasses.modal} style={{ ...styles.card, ...rootStyle }}>Loading checkout...</div>;
  if (!plan) return <div role="alert" className={customClasses.modal} style={{ ...styles.card, ...rootStyle }}>{error || 'Plan not found.'}</div>;

  const targetNetworkName = networkPassphrase === Networks.PUBLIC ? 'Mainnet' : 'Testnet';

  return (
    <section className={`orbit-checkout-root ${customClasses.modal || ''}`} style={{ ...styles.card, ...rootStyle }} aria-label="Orbit checkout">
      <style>{`
        .orbit-checkout-root { isolation: isolate; }
        .orbit-checkout-root .orbit-fee, .orbit-checkout-root .orbit-pending, .orbit-checkout-root .orbit-wrong-network {
          margin: 16px 0; padding: 12px; border: 1px solid var(--orbit-border); border-radius: var(--orbit-radius);
        }
        .orbit-checkout-root .orbit-wrong-network {
          border-color: #f59e0b; background: rgba(245, 158, 11, 0.08);
        }
        .orbit-checkout-root .orbit-confirming { color: var(--orbit-accent); font-weight: 600; }
        .orbit-checkout-root .orbit-field { margin-bottom: 12px; text-align: left; }
        .orbit-checkout-root .orbit-field label { display: block; font-size: 13px; font-weight: 600; margin-bottom: 4px; color: var(--orbit-text); }
        .orbit-checkout-root .orbit-field input {
          width: 100%; box-sizing: border-box; padding: 10px 12px; border: 1px solid var(--orbit-border);
          border-radius: var(--orbit-radius); background: var(--orbit-surface); color: var(--orbit-text); font-family: inherit; font-size: 14px;
        }
        .orbit-checkout-root .orbit-field-error { color: var(--orbit-error); font-size: 12px; margin-top: 4px; }
        @keyframes orbit-in { from { opacity: 0; transform: translateY(4px); } to { opacity: 1; transform: translateY(0); } }
        @media (prefers-reduced-motion: reduce) { .orbit-checkout-root { animation: none; } }
      `}</style>

      <div className={customClasses.header}>
        <p style={styles.eyebrow}>{plan.merchants?.name || 'Orbit merchant'}</p>
        <h2>{plan.name}</h2>
      </div>

      <p style={styles.price}>${(Number(plan.usdc_amount) / 10000000).toFixed(2)} USDC</p>
      <p>Billed every {Math.max(1, Math.round(Number(plan.interval_seconds) / 86400))} days</p>

      {/* Pay With selector and live swap quote (#191) */}
      {address && !receipt && payWithBalances.length > 1 && (
        <div className="orbit-pay-with" style={{ marginTop: 12, marginBottom: 12, textAlign: 'left' }}>
          <label htmlFor="orbit-pay-with" style={{ display: 'block', fontSize: 13, fontWeight: 600, marginBottom: 4 }}>Pay with</label>
          <select
            id="orbit-pay-with"
            value={selectedPayWith ? assetKey(selectedPayWith) : ''}
            onChange={(e) => setPayWithKey(e.target.value)}
            disabled={processing || confirming}
            style={{ width: '100%', padding: '10px 12px', borderRadius: 'var(--orbit-radius)', border: '1px solid var(--orbit-border)', background: 'var(--orbit-surface)', color: 'var(--orbit-text)', fontSize: 14 }}
          >
            {payWithBalances.map((asset) => (
              <option key={assetKey(asset)} value={assetKey(asset)}>
                {asset.code} (balance {Number(asset.balance).toFixed(2)})
              </option>
            ))}
          </select>

          {isSwap && (
            <div role="status" style={{ marginTop: 8, fontSize: 13, color: 'var(--orbit-muted)' }}>
              {swapQuoteLoading && <div>Fetching live swap quote...</div>}
              {swapQuoteError && <div className="orbit-field-error" role="alert">{swapQuoteError}</div>}
              {swapQuote && (
                <>
                  <div>Rate: 1 {PLAN_CURRENCY_CODE} = {swapQuote.rate.toFixed(4)} {selectedPayWith.code}</div>
                  <div>
                    Price impact: {swapQuote.priceImpactPct.toFixed(2)}%
                    {swapQuote.priceImpactPct > HIGH_PRICE_IMPACT_PCT && <strong style={{ color: 'var(--orbit-error)' }}> (high)</strong>}
                  </div>
                  <div>You pay about {swapQuote.sourceAmount} {selectedPayWith.code}, at most {swapQuote.maxInputAmount} after slippage.</div>
                </>
              )}
              <label style={{ display: 'flex', alignItems: 'center', gap: 8, marginTop: 6 }}>
                Slippage tolerance
                <select value={slippagePct} onChange={(e) => setSlippagePct(Number(e.target.value))} disabled={processing || confirming}>
                  {SLIPPAGE_OPTIONS.map((option) => <option key={option} value={option}>{option}%</option>)}
                </select>
              </label>
            </div>
          )}
        </div>
      )}

      {/* Customizable Form Fields */}
      {customFields && customFields.length > 0 && !receipt && (
        <div style={{ marginTop: 16, marginBottom: 16 }}>
          {customFields.map((field) => (
            <div key={field.id} className="orbit-field">
              <label htmlFor={`orbit-field-${field.id}`}>
                {field.label || field.id}
                {field.required && <span style={{ color: 'var(--orbit-error)', marginLeft: 4 }}>*</span>}
              </label>
              <input
                id={`orbit-field-${field.id}`}
                type={field.type || 'text'}
                placeholder={field.placeholder || ''}
                value={formData[field.id] || ''}
                onChange={(e) => handleFieldChange(field.id, e.target.value)}
                disabled={processing || confirming}
              />
              {formErrors[field.id] && (
                <div className="orbit-field-error" role="alert">
                  {formErrors[field.id]}
                </div>
              )}
            </div>
          ))}
        </div>
      )}

      {/* Wrong Network Warning Banner */}
      {isWrongNetwork && (
        <div role="alert" className="orbit-wrong-network">
          <strong style={{ color: '#d97706', display: 'block', marginBottom: 4 }}>Wrong Network Detected</strong>
          <p style={{ margin: '0 0 8px 0', fontSize: 13 }}>
            Your wallet is not connected to Stellar {targetNetworkName}. Please switch networks to proceed with payment.
          </p>
          <button
            style={{
              padding: '8px 14px',
              border: 0,
              borderRadius: 'var(--orbit-radius)',
              background: '#d97706',
              color: '#fff',
              fontWeight: 600,
              fontSize: 13,
              cursor: 'pointer',
            }}
            disabled={switchingNetwork}
            onClick={handleSwitchNetwork}
          >
            {switchingNetwork ? 'Requesting switch...' : `Switch to ${targetNetworkName}`}
          </button>
        </div>
      )}

      {feeEstimate && (
        <div className="orbit-fee" role="status">
          <strong>Estimated network fees</strong><br />
          About {(Number(feeEstimate.estimatedFeeStroops) / 10000000).toFixed(4)} XLM for both transactions.<br />
          <small>USDC balance: {(Number(feeEstimate.usdcBalance) / 10000000).toFixed(2)}. XLM balance: {(Number(feeEstimate.xlmBalance) / 10000000).toFixed(4)}.</small>
        </div>
      )}

      {confirming && (
        <p className="orbit-confirming" role="status">
          Confirming on Stellar {targetNetworkName.toLowerCase()}... {confirmingSeconds}s remaining
        </p>
      )}

      {pending && (
        <div role="status" className="orbit-pending">
          <h3>Transaction Pending</h3>
          <p>Stellar is still processing this transaction. Your funds are not lost.</p>
          <code>{pending.hash}</code>
          <p>
            <a href={`https://stellar.expert/explorer/testnet/tx/${pending.hash}`} target="_blank" rel="noreferrer">
              View on Stellar Expert
            </a>
          </p>
          <button className={customClasses.button} style={styles.button} disabled={processing} onClick={checkPending}>
            Check Again
          </button>
        </div>
      )}

      {receipt ? (
        <div ref={successElement} role="status" aria-live="polite">
          <h3>{receipt.demo ? 'Demo complete - nothing was signed' : receipt.recovered ? 'Vault transaction confirmed' : 'Subscription active'}</h3>
          {receipt.recovered && <p>Check with the merchant that your subscription was recorded. A confirmed transaction alone does not prove the subscription record exists.</p>}
          {receipt.mobile && <p>Mobile wallet approval verified on Stellar.</p>}
          {receipt.vaultHash && (
            <a href={`https://stellar.expert/explorer/testnet/tx/${receipt.vaultHash}`} target="_blank" rel="noreferrer">
              View vault transaction
            </a>
          )}
          <br />
          <button style={styles.cancel} onClick={disconnect}>Start over</button>
        </div>
      ) : (
        !pending &&
        (handoverSession ? (
          <div role="status" aria-live="polite" className="orbit-mobile-handover">
            <h3>Scan with your mobile Stellar wallet</h3>
            <p>Open this checkout in your wallet's mobile DApp browser (LOBSTR or Albedo), connect your wallet and approve the plan.</p>
            <img src={handoverSession.qrDataUrl} alt="QR code to open Orbit checkout on a mobile wallet" width="280" height="280" style={{ maxWidth: '100%', background: '#fff', padding: 8, borderRadius: 8 }} />
            <p style={{ fontSize: 12 }}>This session expires in five minutes. The desktop updates only after the Stellar contract and subscription are verified.</p>
            <a href={handoverSession.url} target="_blank" rel="noopener noreferrer">Open mobile checkout link</a>
            <button style={styles.cancel} onClick={cancelHandover}>Cancel mobile handover</button>
          </div>
        ) : address ? (
          <>
            <p>
              <strong>{isDemo ? 'Demo address' : isSessionKey ? '⚡ Smart Wallet Session' : passkeyAdapter ? 'Passkey account' : 'Wallet'}:</strong> {address.slice(0, 8)}...{address.slice(-5)}
            </p>
            {isSessionKey && (
              <div style={{ marginBottom: 12, padding: '6px 10px', background: 'rgba(16, 185, 129, 0.1)', border: '1px solid rgba(16, 185, 129, 0.3)', borderRadius: 'var(--orbit-radius)', fontSize: 12, color: '#059669', display: 'flex', alignItems: 'center', gap: 6 }}>
                <span>⚡</span>
                <span><strong>Delegated Session Active:</strong> Zero network gas fees (relayer sponsored) & 1-click single confirmation.</span>
              </div>
            )}
            {isDemo && <p role="note">Demo mode never signs transactions or creates a subscription.</p>}
            <button
              className={customClasses.button}
              style={{ ...styles.button, opacity: isWrongNetwork ? 0.6 : 1 }}
              disabled={processing || isWrongNetwork || (isSwap && (!swapQuote || swapQuoteLoading))}
              onClick={subscribe}
            >
              {processing
                ? 'Preparing transaction...'
                : isWrongNetwork
                ? `Switch to ${targetNetworkName} to continue`
                : isDemo
                ? 'Preview success state'
                : isSessionKey
                ? '1-Click Subscribe (Zero Gas)'
                : 'Subscribe & Approve'}
            </button>
            <button style={styles.cancel} onClick={handleOpenWallets}>Switch wallet</button>
            <button style={styles.cancel} onClick={disconnect}>Disconnect</button>
          </>
        ) : (
          <div>
            <button className={customClasses.button} style={styles.button} onClick={handleOpenWallets}>
              Connect wallet
            </button>
            {!incomingChannelId && (
              <button style={{ ...styles.wallet, marginTop: 12 }} disabled={handoverBusy} onClick={startHandover}>
                {handoverBusy ? 'Creating secure QR session…' : 'Scan with Mobile Wallet'}
              </button>
            )}
          </div>
        ))
      )}

      {error && <p role="alert" style={styles.error}>{error}</p>}

      {/* Wallet / Passkey Selection Modal */}
      {showWallets && (
        <div role="dialog" aria-modal="true" aria-labelledby="wallet-title" style={styles.dialog}>
          <h3 id="wallet-title">Choose Payment Method</h3>
          {!incomingChannelId && (
            <button style={{ ...styles.wallet, borderColor: 'var(--orbit-accent)' }} disabled={handoverBusy} onClick={startHandover}>
              {handoverBusy ? 'Creating secure QR session…' : '📱 Scan with Mobile Wallet'}
              <small style={{ display: 'block', color: 'var(--orbit-muted)' }}>Continue in LOBSTR or Albedo on your phone</small>
            </button>
          )}

          {/* Smart Wallet Session Key Option (#169) */}
          {hasSessionKey && (
            <button
              style={{
                ...styles.wallet,
                borderColor: '#10b981',
                background: 'rgba(16, 185, 129, 0.08)',
                marginBottom: 12,
              }}
              onClick={connectSessionKey}
            >
              <div style={{ fontWeight: 700, display: 'flex', alignItems: 'center', gap: 6, color: '#059669' }}>
                <span>⚡ 1-Click Smart Wallet (Zero Gas)</span>
              </div>
              <small style={{ color: 'var(--orbit-muted)' }}>
                Instant onboarding with delegated session key · Zero XLM needed · Gas sponsored by Orbit Relayer
              </small>
            </button>
          )}

          {/* Biometric Passkey Option */}
          {hasPasskey && (
            <button
              style={{
                ...styles.wallet,
                borderColor: 'var(--orbit-accent)',
                background: 'rgba(99, 102, 241, 0.08)',
                marginBottom: 12,
              }}
              onClick={connectPasskey}
            >
              <div style={{ fontWeight: 700, display: 'flex', alignItems: 'center', gap: 6 }}>
                <span>🔑 Pay with Passkey / Biometrics</span>
              </div>
              <small style={{ color: 'var(--orbit-muted)' }}>1-touch checkout with Touch ID, Face ID or Windows Hello</small>
            </button>
          )}

          <div style={{ fontSize: 12, color: 'var(--orbit-muted)', marginBottom: 8, textTransform: 'uppercase', letterSpacing: 0.5 }}>
            Or connect a Stellar wallet
          </div>

          {walletChoices.map(([id, name]) => (
            <button key={id} style={styles.wallet} onClick={() => connect(id)}>
              {name}
            </button>
          ))}

          {demo && (
            <button
              style={styles.wallet}
              onClick={() => {
                setAddress(DEMO_ADDRESS);
                setIsDemo(true);
                setShowWallets(false);
                dispatch('orbit:wallet_connected', { address: DEMO_ADDRESS });
              }}
            >
              Continue in demo mode (no signing)
            </button>
          )}

          <button
            style={styles.cancel}
            onClick={() => {
              setShowWallets(false);
              dispatch('orbit:close');
            }}
          >
            Cancel
          </button>
        </div>
      )}
    </section>
  );
}

const styles = {
  card: {
    fontFamily: 'var(--orbit-font)',
    maxWidth: 440,
    margin: '24px auto',
    padding: 28,
    border: '1px solid var(--orbit-border)',
    borderRadius: 'var(--orbit-radius)',
    background: 'var(--orbit-surface)',
    color: 'var(--orbit-text)',
    position: 'relative',
    animation: 'orbit-in 180ms ease-out',
  },
  eyebrow: { color: 'var(--orbit-muted)', fontSize: 13, textTransform: 'uppercase', letterSpacing: 1 },
  price: { fontSize: 30, fontWeight: 700 },
  button: {
    width: '100%',
    padding: 14,
    border: 0,
    borderRadius: 'var(--orbit-radius)',
    background: 'var(--orbit-accent)',
    color: '#fff',
    fontWeight: 700,
    cursor: 'pointer',
  },
  dialog: {
    position: 'absolute',
    inset: 16,
    padding: 24,
    borderRadius: 'var(--orbit-radius)',
    background: 'var(--orbit-surface)',
    boxShadow: '0 12px 50px #0004',
    zIndex: 2,
    maxHeight: '92%',
    overflowY: 'auto',
  },
  wallet: {
    width: '100%',
    padding: 12,
    marginBottom: 8,
    textAlign: 'left',
    border: '1px solid var(--orbit-border)',
    borderRadius: 'var(--orbit-radius)',
    background: 'var(--orbit-surface)',
    color: 'var(--orbit-text)',
    cursor: 'pointer',
  },
  cancel: { border: 0, background: 'transparent', color: 'var(--orbit-muted)', padding: 8, cursor: 'pointer' },
  error: { color: 'var(--orbit-error)', fontSize: 13 },
};
