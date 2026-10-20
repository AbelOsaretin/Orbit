import { describe, it, expect, beforeEach, vi } from 'vitest';
import { promises as dns } from 'dns';
import {
  validateWebhookURL,
  dispatchSecureWebhook,
  isPrivateOrReservedIP,
  isPrivateIPv4,
  isPrivateIPv6,
  resolveHostname,
  testWebhookSecurity,
} from '../webhook-security.js';

describe("Webhook Security Scanner (#177)", () => {
  beforeEach(() => {
    // Spy on the shared dns.promises object so both this test and the code
    // under test observe the same mocked resolver functions.
    vi.restoreAllMocks();
    vi.spyOn(dns, "resolve4");
    vi.spyOn(dns, "resolve6");
  });

  describe("isPrivateIPv4", () => {
    it("should detect loopback addresses", () => {
      expect(isPrivateIPv4("127.0.0.1")).toBe(true);
      expect(isPrivateIPv4("127.0.0.254")).toBe(true);
    });

    it("should detect RFC 1918 private addresses", () => {
      expect(isPrivateIPv4("10.0.0.1")).toBe(true);
      expect(isPrivateIPv4("172.16.0.1")).toBe(true);
      expect(isPrivateIPv4("192.168.1.1")).toBe(true);
    });

    it("should detect link-local addresses", () => {
      expect(isPrivateIPv4("169.254.169.254")).toBe(true);
    });

    it("should detect carrier-grade NAT addresses", () => {
      expect(isPrivateIPv4("100.64.0.1")).toBe(true);
    });

    it("should detect multicast addresses", () => {
      expect(isPrivateIPv4("224.0.0.1")).toBe(true);
    });

    it("should allow public IPv4 addresses", () => {
      expect(isPrivateIPv4("8.8.8.8")).toBe(false);
      expect(isPrivateIPv4("1.1.1.1")).toBe(false);
      expect(isPrivateIPv4("151.101.1.69")).toBe(false);
    });
  });

  describe("isPrivateIPv6", () => {
    it("should detect loopback addresses", () => {
      expect(isPrivateIPv6("::1")).toBe(true);
    });

    it("should detect link-local addresses", () => {
      expect(isPrivateIPv6("fe80::1")).toBe(true);
    });

    it("should detect unique local addresses", () => {
      expect(isPrivateIPv6("fc00::1")).toBe(true);
      expect(isPrivateIPv6("fd00::1")).toBe(true);
    });

    it("should detect multicast addresses", () => {
      expect(isPrivateIPv6("ff00::1")).toBe(true);
    });

    it("should allow public IPv6 addresses", () => {
      expect(isPrivateIPv6("2606:4700:4700::1111")).toBe(false);
    });
  });

  describe("isPrivateOrReservedIP", () => {
    it("should handle both IPv4 and IPv6", () => {
      expect(isPrivateOrReservedIP("127.0.0.1")).toBe(true);
      expect(isPrivateOrReservedIP("::1")).toBe(true);
      expect(isPrivateOrReservedIP("8.8.8.8")).toBe(false);
    });
  });

  describe("validateWebhookURL", () => {
    it("should reject invalid URLs", async () => {
      await expect(validateWebhookURL("not-a-url")).rejects.toThrow(
        "Invalid URL format",
      );
    });

    it("should reject non-HTTP protocols", async () => {
      await expect(validateWebhookURL("ftp://example.com")).rejects.toThrow(
        "Only HTTP/HTTPS protocols",
      );
      await expect(validateWebhookURL("file:///etc/passwd")).rejects.toThrow(
        "Only HTTP/HTTPS protocols",
      );
    });

    it("should enforce HTTPS in production", async () => {
      await expect(
        validateWebhookURL("http://example.com", true),
      ).rejects.toThrow("must use HTTPS");
    });

    it("should reject URLs with credentials", async () => {
      await expect(
        validateWebhookURL("https://user:pass@example.com"),
      ).rejects.toThrow("authentication credentials");
    });

    it("should reject localhost and loopback", async () => {
      await expect(
        validateWebhookURL("https://localhost/webhook"),
      ).rejects.toThrow("Loopback addresses are not allowed");
      await expect(
        validateWebhookURL("https://127.0.0.1/webhook"),
      ).rejects.toThrow("Loopback addresses are not allowed");
    });

    it("should reject private IP addresses after DNS resolution", async () => {
      dns.resolve4.mockResolvedValue(["10.0.0.1"]);

      await expect(
        validateWebhookURL("https://internal.company.local"),
      ).rejects.toThrow("Private or reserved IP");
    });

    it("should reject link-local and metadata IPs", async () => {
      dns.resolve4.mockResolvedValue(["169.254.169.254"]);

      await expect(
        validateWebhookURL("https://evil.example.com"),
      ).rejects.toThrow("Private or reserved IP");
    });

    it("should accept valid public URLs", async () => {
      dns.resolve4.mockResolvedValue(["151.101.1.69"]);

      const result = await validateWebhookURL(
        "https://api.example.com/webhook",
        false,
      );

      expect(result.valid).toBe(true);
      expect(result.hostname).toBe("api.example.com");
      expect(result.resolvedIPs).toContain("151.101.1.69");
    });

    it("should fallback to IPv6 if IPv4 fails", async () => {
      dns.resolve4.mockRejectedValue(new Error("No IPv4"));
      dns.resolve6.mockResolvedValue(["2606:4700:4700::1111"]);

      const result = await validateWebhookURL(
        "https://api.example.com/webhook",
        false,
      );

      expect(result.valid).toBe(true);
      expect(result.resolvedIPs).toContain("2606:4700:4700::1111");
    });

    it("should reject when DNS resolution fails", async () => {
      dns.resolve4.mockRejectedValue(new Error("NXDOMAIN"));
      dns.resolve6.mockRejectedValue(new Error("NXDOMAIN"));

      await expect(
        validateWebhookURL("https://nonexistent.invalid"),
      ).rejects.toThrow("DNS resolution failed");
    });
  });

  describe("dispatchSecureWebhook", () => {
    let mockFetch;

    beforeEach(() => {
      mockFetch = vi.fn();
      dns.resolve4.mockResolvedValue(["151.101.1.69"]);
    });

    it("should dispatch webhook with validated DNS", async () => {
      mockFetch.mockResolvedValue({
        ok: true,
        status: 200,
        statusText: "OK",
        text: async () => '{"received":true}',
      });

      const result = await dispatchSecureWebhook(
        "https://api.example.com/webhook",
        { event: "subscription.created", data: { id: "123" } },
        { fetchImpl: mockFetch, enforceHTTPS: false },
      );

      expect(result.success).toBe(true);
      expect(result.status).toBe(200);
      expect(mockFetch).toHaveBeenCalledWith(
        "https://api.example.com/webhook",
        expect.objectContaining({
          method: "POST",
          redirect: "error",
        }),
      );
    });

    it("should reject webhooks to private IPs at dispatch time", async () => {
      dns.resolve4.mockResolvedValue(["10.0.0.1"]);

      await expect(
        dispatchSecureWebhook(
          "https://internal.company.local/webhook",
          { event: "test" },
          { fetchImpl: mockFetch, enforceHTTPS: false },
        ),
      ).rejects.toThrow("Private or reserved IP");
    });

    it("should block HTTP redirects", async () => {
      mockFetch.mockResolvedValue({
        ok: true,
        status: 200,
        statusText: "OK",
        text: async () => "{}",
      });

      await dispatchSecureWebhook(
        "https://api.example.com/webhook",
        { event: "test" },
        { fetchImpl: mockFetch, enforceHTTPS: false },
      );

      expect(mockFetch).toHaveBeenCalledWith(
        expect.any(String),
        expect.objectContaining({
          redirect: "error", // Redirects are blocked
        }),
      );
    });

    it("should timeout long-running requests", async () => {
      // Behaves like real fetch: rejects when the abort signal fires
      mockFetch.mockImplementation((_, { signal }) => new Promise((_, reject) => {
        signal.addEventListener("abort", () =>
          reject(Object.assign(new Error("This operation was aborted"), { name: "AbortError" })),
        );
      }));

      await expect(
        dispatchSecureWebhook(
          "https://api.example.com/webhook",
          { event: "test" },
          { fetchImpl: mockFetch, timeout: 100, enforceHTTPS: false },
        ),
      ).rejects.toThrow("timeout");
    }, 10000);

    it("should include custom headers", async () => {
      mockFetch.mockResolvedValue({
        ok: true,
        status: 200,
        statusText: "OK",
        text: async () => "{}",
      });

      await dispatchSecureWebhook(
        "https://api.example.com/webhook",
        { event: "test" },
        {
          fetchImpl: mockFetch,
          enforceHTTPS: false,
          headers: { "X-Custom-Header": "value" },
        },
      );

      expect(mockFetch).toHaveBeenCalledWith(
        expect.any(String),
        expect.objectContaining({
          headers: expect.objectContaining({
            "X-Custom-Header": "value",
          }),
        }),
      );
    });
  });

  describe("testWebhookSecurity", () => {
    it("should return safe status for public URLs", async () => {
      dns.resolve4.mockResolvedValue(["151.101.1.69"]);

      const result = await testWebhookSecurity(
        "https://api.example.com/webhook",
      );

      expect(result.safe).toBe(true);
      expect(result.resolvedIPs).toBeDefined();
    });

    it("should return unsafe status for private URLs", async () => {
      dns.resolve4.mockResolvedValue(["10.0.0.1"]);

      const result = await testWebhookSecurity("https://internal.local");

      expect(result.safe).toBe(false);
      expect(result.error).toContain("Private or reserved IP");
    });
  });

  describe("DNS Rebinding Protection", () => {
    it("should validate URL at both registration and dispatch", async () => {
      // Registration time: resolve to public IP
      dns.resolve4.mockResolvedValue(["151.101.1.69"]);
      const validation = await validateWebhookURL(
        "https://evil.example.com",
        false,
      );
      expect(validation.valid).toBe(true);

      // Dispatch time: DNS rebinding attack - now resolves to private IP
      dns.resolve4.mockResolvedValue(["127.0.0.1"]);

      const mockFetch = vi.fn();
      await expect(
        dispatchSecureWebhook(
          "https://evil.example.com",
          { event: "test" },
          { fetchImpl: mockFetch, enforceHTTPS: false },
        ),
      ).rejects.toThrow();
    });
  });
});
