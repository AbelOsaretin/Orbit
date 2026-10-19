import { afterAll, describe, expect, it, vi } from 'vitest';
import { createClient } from 'graphql-ws';
import WebSocket from 'ws';
import { createServer } from 'node:http';
import { Keypair } from '@stellar/stellar-sdk';
import { createRequire } from 'node:module';
import { makeApiKey } from './helpers.js';

const require = createRequire(import.meta.url);
const app = require('../index.js');
const merchantId = 'e070b874-069f-4de2-af22-4556ce32c9b4';
const otherMerchantId = 'f8e03014-07cf-4054-a553-bb201bfb3238';
const wallet = Keypair.random().publicKey();
const key = makeApiKey({ merchantId, walletAddress: wallet });
let server, wsClosed = false;
const bus = app.locals.paymentEventBus;

const db = {
  from(table) {
    return {
      select() { return this; },
      eq() { return this; },
      async maybeSingle() {
        return { error: null, data: table === 'api_keys' ? key.row : null };
      },
    };
  },
};
app.locals.supabase = db;

async function setupSocket() {
  server = createServer(app);
  app.locals.graphqlServer.attachWebsocket(server);
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  return `ws://127.0.0.1:${server.address().port}/graphql`;
}
async function waitForEvent(client, input) {
  return new Promise((resolve, reject) => {
    client.subscribe(input, {
      next: resolve,
      error: reject,
      complete: () => reject(new Error('Stream completed without event')),
    });
  });
}

describe('authenticated GraphQL WS subscriptions', () => {
  afterAll(async () => {
    if (!wsClosed && app.locals.graphqlServer.closeWebsocket) {
      wsClosed = true;
      await app.locals.graphqlServer.closeWebsocket();
    }
    if (server) await new Promise((resolve) => server.close(resolve));
    // Apollo server may be shared with the existing graphql.test.js suite.
  });

  it('publishes merchant settlement data over a connected authenticated socket', async () => {
    const url = await setupSocket();
    const originalSubscribe = bus.subscribe;
    let streamReady;
    const subscribed = new Promise((resolve) => { streamReady = resolve; });
    bus.subscribe = (...args) => {
      const iterator = originalSubscribe(...args);
      streamReady();
      return iterator;
    };
    let connected;
    const ready = new Promise((resolve) => { connected = resolve; });
    const client = createClient({
      url, webSocketImpl: WebSocket, connectionParams: { authorization: key.header },
      retryAttempts: 0,
      on: { connected: () => connected() },
    });
    try {
      const eventPromise = waitForEvent(client, {
        query: 'subscription($id: ID!) { subscriptionSettlementFeed(merchantId: $id) { id merchantId txHash eventType status } }',
        variables: { id: merchantId },
      });
      await ready;
      await subscribed;
      await bus.publish('paymentSettled', {
        id: 'hash-123', txHash: 'hash-123', merchantId,
        status: 'settled', subscriptionId: 'subscription-3',
      });
      const result = await Promise.race([
        eventPromise,
        new Promise((_, reject) => setTimeout(() => reject(new Error('WS event not delivered')), 3000)),
      ]);
      expect(result.errors).toBeUndefined();
      expect(result.data.subscriptionSettlementFeed).toMatchObject({
        id: 'hash-123', txHash: 'hash-123', merchantId, eventType: 'paymentSettled',
      });
    } finally {
      bus.subscribe = originalSubscribe;
      await client.dispose();
    }
  });

  it('rejects a cross-merchant subscription on the same authenticated connection', async () => {
    const port = server.address().port;
    const client = createClient({
      url: `ws://127.0.0.1:${port}/graphql`,
      webSocketImpl: WebSocket, connectionParams: { authorization: key.header }, retryAttempts: 0,
    });
    try {
      const response = await Promise.race([
        waitForEvent(client, {
          query: 'subscription($id: ID!) { subscriptionSettlementFeed(merchantId: $id) { id } }',
          variables: { id: otherMerchantId },
        }).catch((error) => ({ errors: error })),
        new Promise((_, reject) => setTimeout(() => reject(new Error('WS scope check timed out')), 3000)),
      ]);
      expect(response.errors).toBeDefined();
    } finally { await client.dispose(); }
  });

  it('requires an API key in the WebSocket initial connection', async () => {
    const port = server.address().port;
    const client = createClient({
      url: `ws://127.0.0.1:${port}/graphql`, webSocketImpl: WebSocket,
      retryAttempts: 0, connectionParams: {},
    });
    const response = await Promise.race([
      waitForEvent(client, {
        query: 'subscription { subscriptionSettlementFeed(merchantId: "x") { id } }',
      }).catch((error) => error),
      new Promise((_, reject) => setTimeout(() => reject(new Error('WS anonymous connection timed out')), 3000)),
    ]);
    expect(response).toBeTruthy();
    await client.dispose();
  });
});
