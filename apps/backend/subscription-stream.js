const crypto = require('crypto');
const { EventEmitter } = require('events');

const EVENT_TYPES = new Set(['subscriptionCreated', 'paymentSettled', 'vaultCancelled']);
const STREAM_PREFIX = 'orbit:subscription-stream:';

/**
 * Merchant-scoped pub/sub fan-out. Redis supports multiple API instances;
 * without REDIS_URL the in-process bus is used (local development and tests).
 * A bounded queue prevents disconnected/slow subscribers from leaking memory.
 */
function createSubscriptionEventBus({ redisUrl = process.env.REDIS_URL, RedisClient, maxQueue = 128 } = {}) {
    const emitter = new EventEmitter();
    const activeStreams = new Set();
    emitter.setMaxListeners(0);
    const instance = crypto.randomUUID();
    let publisher, subscriber;
    let stopped = false;
    if (redisUrl) {
        try {
            const Redis = RedisClient || require('ioredis');
            publisher = new Redis(redisUrl);
            subscriber = new Redis(redisUrl);
            publisher.on('error', (error) => console.error('Subscription publisher Redis error:', error.message));
            subscriber.on('error', (error) => console.error('Subscription subscriber Redis error:', error.message));
            subscriber.on('pmessage', (_pattern, channel, raw) => {
                try {
                    if (!channel.startsWith(STREAM_PREFIX)) return;
                    const { origin, event } = JSON.parse(raw);
                    if (origin === instance || !EVENT_TYPES.has(event?.eventType) ||
                        event.merchantId !== channel.slice(STREAM_PREFIX.length)) return;
                    emitter.emit(event.merchantId, event);
                } catch (error) {
                    console.error('Ignoring malformed Redis subscription event:', error.message);
                }
            });
            subscriber.psubscribe(STREAM_PREFIX + '*').catch((error) =>
                console.error('Subscription Redis pattern subscribe failed:', error.message));
        } catch (error) {
            publisher = undefined;
            subscriber = undefined;
            console.error('Subscription Redis unavailable, using local events:', error.message);
        }
    }

    async function publish(eventType, data) {
        if (stopped) throw new Error('Subscription event bus has stopped');
        if (!EVENT_TYPES.has(eventType)) throw new Error('Unsupported subscription event');
        if (!data || typeof data.merchantId !== 'string' || !data.merchantId) {
            throw new Error('A merchant ID is required');
        }
        const event = {
            ...data,
            id: data.id || crypto.randomUUID(),
            eventType,
            merchantId: data.merchantId,
            settledAt: data.settledAt || new Date().toISOString(),
        };
        emitter.emit(event.merchantId, event);
        if (publisher) {
            try {
                await publisher.publish(STREAM_PREFIX + event.merchantId, JSON.stringify({ origin: instance, event }));
            } catch (error) {
                console.error('Subscription Redis publish failed:', error.message);
            }
        }
        return event;
    }

    function subscribe(merchantId) {
        if (stopped || !merchantId) throw new Error('Missing merchant ID or bus stopped');
        let closed = false;
        const queue = [];
        const waiters = [];
        const onMessage = (event) => {
            if (closed) return;
            const value = { subscriptionSettlementFeed: event };
            if (waiters.length) waiters.shift()({ value, done: false });
            else {
                if (queue.length >= maxQueue) queue.shift();
                queue.push(value);
            }
        };
        emitter.on(merchantId, onMessage);
        const close = () => {
            if (closed) return;
            closed = true;
            emitter.off(merchantId, onMessage);
            activeStreams.delete(iterator);
            queue.length = 0;
            for (const resolve of waiters.splice(0)) resolve({ value: undefined, done: true });
        };
        const iterator = {
            async next() {
                if (queue.length) return { value: queue.shift(), done: false };
                if (closed) return { value: undefined, done: true };
                return new Promise((resolve) => waiters.push(resolve));
            },
            async return() {
                close();
                return { value: undefined, done: true };
            },
            [Symbol.asyncIterator]() { return this; },
        };
        activeStreams.add(iterator);
        return iterator;
    }

    async function stop() {
        stopped = true;
        for (const stream of activeStreams) await stream.return();
        emitter.removeAllListeners();
        await Promise.allSettled([publisher?.quit(), subscriber?.quit()].filter(Boolean));
    }
    return { publish, subscribe, stop };
}

module.exports = { createSubscriptionEventBus, EVENT_TYPES };
