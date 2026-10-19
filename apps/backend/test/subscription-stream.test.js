import { afterEach, describe, expect, it } from 'vitest';
import { createRequire } from 'module';

const require = createRequire(import.meta.url);
const { createSubscriptionEventBus } = require('../subscription-stream.js');
const buses = [];
const bus = (options) => {
  const value = createSubscriptionEventBus({ redisUrl: '', ...options });
  buses.push(value);
  return value;
};

afterEach(async () => {
  await Promise.all(buses.splice(0).map((entry) => entry.stop()));
});

describe('GraphQL subscription event fanout', () => {
  it('delivers a verified settlement to the correct merchant stream', async () => {
    const stream = bus().subscribe('merchant-1');
    const published = await buses[0].publish('paymentSettled', {
      merchantId: 'merchant-1', subscriptionId: 'sub-1',
      amount: '10000000', status: 'settled', txHash: 'stellarhash-1',
    });
    const received = await stream.next();
    expect(received.done).toBe(false);
    expect(received.value.subscriptionSettlementFeed).toMatchObject({
      eventType: 'paymentSettled', merchantId: 'merchant-1',
      subscriptionId: 'sub-1', amount: '10000000', txHash: 'stellarhash-1',
      id: published.id,
    });
    await stream.return();
  });

  it('isolates tenants and never sends another merchant\'s messages', async () => {
    const eventBus = bus();
    const first = eventBus.subscribe('first');
    const second = eventBus.subscribe('second');
    await eventBus.publish('subscriptionCreated', { merchantId: 'first', subscriptionId: 's1' });
    await eventBus.publish('vaultCancelled', { merchantId: 'second', subscriptionId: 's2' });
    expect((await first.next()).value.subscriptionSettlementFeed.subscriptionId).toBe('s1');
    expect((await second.next()).value.subscriptionSettlementFeed.subscriptionId).toBe('s2');
    await first.return();
    await second.return();
  });

  it('bounds queued messages and settles pending listeners on disconnect', async () => {
    const eventBus = bus({ maxQueue: 2 });
    const stream = eventBus.subscribe('merchant');
    for (const id of ['a', 'b', 'c']) {
      await eventBus.publish('paymentSettled', { merchantId: 'merchant', id });
    }
    expect((await stream.next()).value.subscriptionSettlementFeed.id).toBe('b');
    expect((await stream.next()).value.subscriptionSettlementFeed.id).toBe('c');
    const pending = stream.next();
    await stream.return();
    expect((await pending).done).toBe(true);
    expect((await stream.next()).done).toBe(true);
  });

  it('rejects unsupported event types and unscoped events', async () => {
    const eventBus = bus();
    await expect(eventBus.publish('unverifiedTransfer', { merchantId: 'm' })).rejects.toThrow();
    await expect(eventBus.publish('paymentSettled', {})).rejects.toThrow();
  });
});
