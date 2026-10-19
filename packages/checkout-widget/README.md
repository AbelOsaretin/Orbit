# Orbit Checkout Widget

`<OrbitCheckout />` is an embeddable React checkout for Orbit pull payments on Stellar Soroban. It loads a plan, lets the customer choose a supported wallet, signs the USDC `approve` and Orbit `create_vault` transactions, waits for both to confirm, and only then records the subscription with the Merchant API.

## Supported wallets

The widget configures only these tree-shakeable modules from Stellar Wallets Kit:

- Freighter
- Albedo
- Lobstr
- Hana
- xBull

The selected wallet is persisted in `localStorage`. Use `allowedWallets` to expose a subset.

## Setup

```bash
cd packages/checkout-widget
npm install
npm run dev
```

Configure the public Vite environment:

```dotenv
VITE_ORBIT_CONTRACT_ID=C...
VITE_USDC_CONTRACT_ID=C...
VITE_SOROBAN_RPC_URL=https://soroban-testnet.stellar.org
VITE_STELLAR_NETWORK_PASSPHRASE=Test SDF Network ; September 2015
```

## Props

| Prop | Default | Description |
|---|---|---|
| `planId` | — | Plan UUID loaded from `GET /plans/:id`. |
| `planData` | — | Preloaded plan; must include the merchant wallet address. |
| `apiUrl` | `http://localhost:3001` | Merchant API base URL. |
| `allowedWallets` | all five wallets | Array of Stellar Wallets Kit wallet IDs. |
| `demo` | `false` | Adds an explicitly labelled offline preview option. It never signs or records a subscription. |
| `orbitContractId` | `VITE_ORBIT_CONTRACT_ID` | Orbit contract address. |
| `tokenContractId` | `VITE_USDC_CONTRACT_ID` | USDC SAC address. |
| `rpcUrl` | Stellar testnet RPC | Soroban JSON-RPC endpoint. |
| `networkPassphrase` | Stellar testnet | Network used for signing. |
| `theme` | `auto` | `dark`, `light`, or `auto`. |
| `accentColor` | `#6366f1` | Hex color for buttons and active states. |
| `borderRadius` | `md` | `none`, `sm`, `md`, `lg`, or `full`. |
| `fontFamily` | `inherit` | CSS font family or font stack. |
| `customClasses` | `{}` | Optional `{ modal, header, button }` class names. |
| `allowedOrigins` | `[]` | Trusted parent origins for lifecycle `postMessage` events. Empty permits same-origin parent only. |

```jsx
<OrbitCheckout
  planId="e2a0b3df-2015-4672-9721-a1e94c1408d7"
  apiUrl="http://localhost:3001"
  allowedWallets={["freighter", "albedo", "lobstr", "hana", "xbull"]}
/>
```

The allowance covers 12 billing cycles and expires after those cycles plus a ledger safety margin. If either wallet transaction fails or is rejected, `POST /subscriptions` is not called and the error remains visible in the widget.

Before signatures, the widget simulates the USDC `approve` and Orbit `create_vault` transactions, reads USDC/XLM balances, and displays estimated fees. In-flight hashes are backed up in local storage and can be checked again after a refresh or a long network delay.

## Lifecycle events

Events are sent to the embedding parent with `window.postMessage`. Set explicit `allowedOrigins` values for cross-origin embeds.

| Event | Payload |
|---|---|
| `orbit:ready` | `{ type, planId }` |
| `orbit:wallet_connected` | `{ type, address }` |
| `orbit:subscribed` | `{ type, subscriptionId, planId, txHash }` |
| `orbit:error` | `{ type, message }` |
| `orbit:close` | `{ type }` |

```js
window.addEventListener('message', ({ data, origin }) => {
  if (data.type === 'orbit:subscribed') provisionCustomer(data.planId, data.txHash, origin);
});
```


## Desktop-to-mobile QR handover (#210)

The checkout now offers **Scan with Mobile Wallet** alongside browser wallet selection. It opens a server-backed session with 24 bytes of cryptographically secure randomness and a strict five-minute expiry. The QR contains a URL back to the **same live checkout page**, with an `orbit_handover` query parameter. The widget on the mobile page reloads the plan from the server, letting the customer connect using a supported Stellar mobile DApp browser (e.g. the LOBSTR app browser or Albedo mobile flow).

**Integration requirement:** The QR's destination must be reachable from a phone, must render the same `<OrbitCheckout />` widget, and must be able to access the Merchant API using HTTPS. An `http://localhost` merchant page works only for local single-device testing. This is a web handover, not a fabricated WalletConnect pairing or unsupported wallet-native transaction URI.

After mobile wallet signing succeeds, the API first records a subscription, then `POST /handover/sessions/:id/complete` checks the Stellar RPC transaction **SUCCESS**, its ledger, vault-creation event, token, payout amount, interval, merchant, and customer address against the server's plan and subscription records. Only then does the desktop polling flow show a GSAP-animated receipt. Session state expires after five minutes; the widget stops polling cleanly.

The backend requires `ORBIT_CONTRACT_ID`, `USDC_CONTRACT_ID`, `SOROBAN_RPC_URL` and a working payment database. Set `REDIS_URL` in horizontally scaled environments so independent API replicas share the expiring session state. Otherwise the built-in memory fallback is **single-process only**.

Run `npm test` (unit tests) and `npm run build` for this package. Handover approval requires a real wallet transaction; offline/demo states cannot mark a session confirmed.
