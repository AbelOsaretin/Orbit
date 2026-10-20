const dns = require("dns").promises;
const { URL } = require("url");
const http = require("http");
const https = require("https");

/**
 * SSRF Defense and Webhook Security Scanner (#177)
 *
 * Implements comprehensive Server-Side Request Forgery (SSRF) protection for webhook
 * destination endpoints through DNS resolution, IP filtering, and DNS rebinding defense.
 */

// RFC 1918 private IPv4 ranges
const PRIVATE_IPV4_RANGES = [
  { start: [10, 0, 0, 0], end: [10, 255, 255, 255] }, // 10.0.0.0/8
  { start: [172, 16, 0, 0], end: [172, 31, 255, 255] }, // 172.16.0.0/12
  { start: [192, 168, 0, 0], end: [192, 168, 255, 255] }, // 192.168.0.0/16
  { start: [127, 0, 0, 0], end: [127, 255, 255, 255] }, // 127.0.0.0/8 (loopback)
  { start: [169, 254, 0, 0], end: [169, 254, 255, 255] }, // 169.254.0.0/16 (link-local)
  { start: [100, 64, 0, 0], end: [100, 127, 255, 255] }, // 100.64.0.0/10 (carrier-grade NAT)
  { start: [224, 0, 0, 0], end: [239, 255, 255, 255] }, // 224.0.0.0/4 (multicast)
  { start: [240, 0, 0, 0], end: [255, 255, 255, 255] }, // 240.0.0.0/4 (reserved)
];

// Private/reserved IPv6 ranges
const PRIVATE_IPV6_PREFIXES = [
  "::1", // loopback
  "fe80:", // link-local
  "fc00:", // unique local addresses (ULA)
  "fd00:", // unique local addresses (ULA)
  "ff00:", // multicast
  "::ffff:127.", // IPv4-mapped loopback
  "::ffff:10.", // IPv4-mapped private
  "::ffff:172.", // IPv4-mapped private
  "::ffff:192.168.", // IPv4-mapped private
];

/**
 * Parse IPv4 address to array of octets
 */
function parseIPv4(ip) {
  const octets = ip.split(".").map(Number);
  if (octets.length !== 4 || octets.some((o) => isNaN(o) || o < 0 || o > 255)) {
    return null;
  }
  return octets;
}

/**
 * Check if IPv4 address is in a specific range
 */
function isIPv4InRange(octets, range) {
  for (let i = 0; i < 4; i++) {
    if (octets[i] < range.start[i]) return false;
    if (octets[i] > range.end[i]) return false;
    if (octets[i] > range.start[i] && octets[i] < range.end[i]) return true;
  }
  return true;
}

/**
 * Check if an IPv4 address is private/reserved
 */
function isPrivateIPv4(ip) {
  const octets = parseIPv4(ip);
  if (!octets) return false;

  return PRIVATE_IPV4_RANGES.some((range) => isIPv4InRange(octets, range));
}

/**
 * Check if an IPv6 address is private/reserved
 */
function isPrivateIPv6(ip) {
  const normalized = ip.toLowerCase();
  return PRIVATE_IPV6_PREFIXES.some((prefix) => normalized.startsWith(prefix));
}

/**
 * Check if an IP address (v4 or v6) is private, loopback, or reserved
 */
function isPrivateOrReservedIP(ip) {
  if (ip.includes(":")) {
    return isPrivateIPv6(ip);
  }
  return isPrivateIPv4(ip);
}

/**
 * Resolve hostname to IP addresses
 */
async function resolveHostname(hostname) {
  try {
    // Try IPv4 first
    const ipv4 = await dns.resolve4(hostname);
    return ipv4;
  } catch (err) {
    // Fallback to IPv6
    try {
      const ipv6 = await dns.resolve6(hostname);
      return ipv6;
    } catch (err2) {
      throw new Error(`DNS resolution failed for ${hostname}: ${err2.message}`);
    }
  }
}

/**
 * Validate webhook URL at registration time
 * Performs DNS resolution and IP filtering before persisting the endpoint
 */
async function validateWebhookURL(url, enforceHTTPS = true) {
  let parsedURL;

  try {
    parsedURL = new URL(url);
  } catch (err) {
    throw new Error("Invalid URL format");
  }

  // Reject file://, ftp://, gopher://, etc.
  if (!["http:", "https:"].includes(parsedURL.protocol)) {
    throw new Error("Only HTTP/HTTPS protocols are allowed");
  }

  // Enforce HTTPS in production
  if (enforceHTTPS && parsedURL.protocol !== "https:") {
    throw new Error("Webhook URLs must use HTTPS protocol in production");
  }

  // Reject URLs with authentication credentials
  if (parsedURL.username || parsedURL.password) {
    throw new Error("URLs with authentication credentials are not allowed");
  }

  const hostname = parsedURL.hostname;

  // Reject localhost and known dangerous hostnames
  const dangerousHosts = ["localhost", "0.0.0.0", "127.0.0.1", "[::1]"];
  if (dangerousHosts.includes(hostname.toLowerCase())) {
    throw new Error("Loopback addresses are not allowed");
  }

  // Resolve hostname to IP addresses
  let ips;
  try {
    ips = await resolveHostname(hostname);
  } catch (err) {
    throw new Error(`DNS resolution failed: ${err.message}`);
  }

  // Check all resolved IPs
  for (const ip of ips) {
    if (isPrivateOrReservedIP(ip)) {
      throw new Error(`Private or reserved IP address detected: ${ip}`);
    }
  }

  return {
    valid: true,
    url: parsedURL.toString(),
    hostname,
    resolvedIPs: ips,
  };
}

/**
 * Create a custom HTTP/HTTPS agent with DNS rebinding protection
 * Pins the validated IP address for the connection socket
 */
function createSecureAgent(hostname, validatedIP, protocol = "https") {
  const AgentClass = protocol === "https" ? https.Agent : http.Agent;

  return new AgentClass({
    lookup: (hostname, options, callback) => {
      // Pin to the validated IP, preventing DNS rebinding attacks
      callback(null, validatedIP, options.family || 4);
    },
    timeout: 6000,
    maxSockets: 1,
  });
}

/**
 * Dispatch webhook with SSRF protection
 * Performs DNS resolution at execution time and blocks redirects
 */
async function dispatchSecureWebhook(url, payload, options = {}) {
  const {
    headers = {},
    timeout = 6000,
    enforceHTTPS = true,
    fetchImpl = globalThis.fetch,
  } = options;

  // Validate URL at dispatch time (DNS rebinding defense)
  let validation;
  try {
    validation = await validateWebhookURL(url, enforceHTTPS);
  } catch (err) {
    throw new Error(`Webhook URL validation failed: ${err.message}`);
  }

  const { hostname, resolvedIPs } = validation;
  const validatedIP = resolvedIPs[0];

  const parsedURL = new URL(url);
  const protocol = parsedURL.protocol === "https:" ? "https" : "http";

  // Create agent with DNS pinning
  const agent = createSecureAgent(hostname, validatedIP, protocol);

  // Prepare request options
  const controller = new AbortController();
  const timeoutId = setTimeout(() => controller.abort(), timeout);

  try {
    const response = await fetchImpl(url, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        ...headers,
      },
      body: JSON.stringify(payload),
      signal: controller.signal,
      redirect: "error", // Block all HTTP redirects
      agent, // Use custom agent with DNS pinning
    });

    clearTimeout(timeoutId);

    return {
      success: response.ok,
      status: response.status,
      statusText: response.statusText,
      body: await response.text().catch(() => ""),
    };
  } catch (err) {
    clearTimeout(timeoutId);

    if (err.name === "AbortError") {
      throw new Error("Webhook request timeout");
    }

    throw new Error(`Webhook dispatch failed: ${err.message}`);
  }
}

/**
 * Test webhook endpoint security (for unit tests)
 */
async function testWebhookSecurity(url) {
  try {
    const validation = await validateWebhookURL(url, false);
    return {
      safe: true,
      ...validation,
    };
  } catch (err) {
    return {
      safe: false,
      error: err.message,
    };
  }
}

module.exports = {
  validateWebhookURL,
  dispatchSecureWebhook,
  createSecureAgent,
  isPrivateOrReservedIP,
  resolveHostname,
  testWebhookSecurity,
  // Export for testing
  isPrivateIPv4,
  isPrivateIPv6,
};
