import QRCode from 'qrcode';

export const HANDOVER_EXPIRY_MS = 5 * 60 * 1000;
export const HANDOVER_POLL_MS = 1500;
const CHANNEL_PATTERN = /^[0-9a-f]{48}$/i;

export function mobileHandoverUrl(pageUrl, channelId) {
  if (!CHANNEL_PATTERN.test(channelId)) throw new Error('Invalid mobile handover channel');
  const url = new URL(pageUrl);
  if (!['http:', 'https:'].includes(url.protocol)) throw new Error('Handover requires an HTTP(S) checkout URL');
  url.searchParams.set('orbit_handover', channelId);
  return url.toString();
}

export async function handoverQRCode(url) {
  return QRCode.toDataURL(url, {
    width: 400, margin: 2, errorCorrectionLevel: 'Q',
    color: { dark: '#111827', light: '#ffffff' },
  });
}

async function jsonResponse(response) {
  let data;
  try { data = await response.json(); } catch { throw new Error('The mobile handover service returned an invalid response.'); }
  if (!response.ok) throw new Error(data?.error || 'Mobile handover request failed.');
  return data;
}

export async function openHandover({ apiUrl, planId, pageUrl, fetchImpl = fetch }) {
  const response = await fetchImpl(`${apiUrl}/handover/sessions`, {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ plan_id: planId }),
  });
  const { session } = await jsonResponse(response);
  if (!session || !CHANNEL_PATTERN.test(session.id) || session.planId !== planId ||
      !Number.isFinite(session.expiresAt)) throw new Error('Malformed handover session');
  const url = mobileHandoverUrl(pageUrl, session.id);
  return { ...session, url, qrDataUrl: await handoverQRCode(url) };
}

export async function readHandover({ apiUrl, channelId, fetchImpl = fetch }) {
  if (!CHANNEL_PATTERN.test(channelId)) throw new Error('Invalid handover channel');
  const response = await fetchImpl(`${apiUrl}/handover/sessions/${channelId}`, {
    headers: { Accept: 'application/json' }, cache: 'no-store',
  });
  if (response.status === 410) return { status: 'expired' };
  const { session } = await jsonResponse(response);
  return session;
}

export async function completeHandover({ apiUrl, channelId, txHash, subscriptionId, customerWalletAddress, fetchImpl = fetch }) {
  if (!CHANNEL_PATTERN.test(channelId)) throw new Error('Invalid handover channel');
  const response = await fetchImpl(`${apiUrl}/handover/sessions/${channelId}/complete`, {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      tx_hash: txHash, subscription_id: subscriptionId,
      customer_wallet_address: customerWalletAddress,
    }),
  });
  const { session } = await jsonResponse(response);
  if (session?.status !== 'confirmed' || session?.confirmation?.txHash !== txHash) {
    throw new Error('No verified on-chain handover receipt returned.');
  }
  return session;
}

export function watchHandover({
  apiUrl, session, onConfirmed, onExpired, onError, fetchImpl = fetch,
  pollMs = HANDOVER_POLL_MS, clock = Date.now,
}) {
  if (!session?.id || !Number.isFinite(session.expiresAt)) throw new Error('Invalid session');
  let active = true;
  let busy = false;
  let intervalId;
  const stop = () => { active = false; if (intervalId) clearInterval(intervalId); };
  const poll = async () => {
    if (!active || busy) return;
    if (clock() >= session.expiresAt) { stop(); onExpired?.(); return; }
    busy = true;
    try {
      const state = await readHandover({ apiUrl, channelId: session.id, fetchImpl });
      if (!active) return;
      if (state.status === 'confirmed') {
        const receipt = state.confirmation;
        if (receipt?.txHash && receipt?.subscriptionId) { stop(); onConfirmed?.(receipt); }
        else onError?.(new Error('The handover receipt is incomplete.'));
      } else if (state.status === 'expired') { stop(); onExpired?.(); }
    } catch (error) {
      if (active) onError?.(error);
    } finally { busy = false; }
  };
  intervalId = setInterval(poll, pollMs);
  poll();
  return { stop };
}
