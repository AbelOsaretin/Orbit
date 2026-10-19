import test from 'node:test';
import assert from 'node:assert/strict';
import {
  HANDOVER_EXPIRY_MS, mobileHandoverUrl,
  readHandover, completeHandover, watchHandover,
} from '../src/mobileHandover.js';

const channel = 'a'.repeat(48);
const apiUrl = 'https://api.example.com';

const fetchResponse = (payload, status = 200) => ({
  ok: status >= 200 && status < 300,
  status,
  async json() { return payload; },
});

test('handover URLs preserve existing checkout parameters and never switch origin', () => {
  const value = mobileHandoverUrl('https://merchant.example/checkout?plan=one', channel);
  const result = new URL(value);
  assert.equal(result.origin, 'https://merchant.example');
  assert.equal(result.searchParams.get('plan'), 'one');
  assert.equal(result.searchParams.get('orbit_handover'), channel);
  assert.throws(() => mobileHandoverUrl('javascript:alert(1)', channel));
  assert.throws(() => mobileHandoverUrl('https://merchant.example', '../bad'));
});

test('receipt API never trusts a client-provided success flag', async () => {
  let call;
  const mockFetch = async (url, options) => {
    call = { url, options };
    return fetchResponse({ session: { status: 'confirmed', confirmation: { txHash: 'b'.repeat(64) } } });
  };
  const result = await completeHandover({
    apiUrl, channelId: channel, txHash: 'b'.repeat(64),
    subscriptionId: 'sub-1', customerWalletAddress: 'WALLET', fetchImpl: mockFetch,
  });
  assert.equal(result.status, 'confirmed');
  const sent = JSON.parse(call.options.body);
  assert.equal(sent.subscription_id, 'sub-1');
  assert.equal(sent.tx_hash, 'b'.repeat(64));
  await assert.rejects(() => completeHandover({
    apiUrl, channelId: channel, txHash: 'c'.repeat(64),
    fetchImpl: mockFetch,
  }), /verified on-chain handover receipt/i);
});

test('expired handover is reported cleanly', async () => {
  assert.deepEqual(await readHandover({
    apiUrl, channelId: channel,
    fetchImpl: async () => fetchResponse({ session: { status: 'expired' } }, 410),
  }), { status: 'expired' });
});

test('desktop watcher confirms once and stops on verified server state', async () => {
  let calls = 0; const events = [];
  const stop = watchHandover({
    apiUrl, session: { id: channel, expiresAt: Date.now() + HANDOVER_EXPIRY_MS },
    pollMs: 10,
    fetchImpl: async () => {
      calls += 1;
      return fetchResponse({ session: {
        status: 'confirmed',
        confirmation: { txHash: 'd'.repeat(64), subscriptionId: 'sub-42' },
      } });
    },
    onConfirmed: (receipt) => events.push(receipt),
  });
  await new Promise((resolve) => setTimeout(resolve, 50));
  stop.stop();
  assert.equal(events.length, 1);
  assert.equal(events[0].subscriptionId, 'sub-42');
  assert.equal(calls, 1);
});

test('desktop watcher expires instead of continuously polling an old channel', async () => {
  let expired = 0;
  const stop = watchHandover({
    apiUrl, session: { id: channel, expiresAt: 10 }, clock: () => 11,
    pollMs: 10, onExpired: () => { expired++; },
    fetchImpl: async () => { throw new Error('should not poll'); },
  });
  await new Promise((resolve) => setTimeout(resolve, 25));
  stop.stop();
  assert.equal(expired, 1);
});
