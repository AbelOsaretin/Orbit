import {
  Address,
  BASE_FEE,
  Contract,
  Networks,
  StrKey,
  TransactionBuilder,
  nativeToScVal,
  scValToNative,
  rpc,
  xdr,
} from "@stellar/stellar-sdk";
import { getAddress, isConnected, requestAccess, signTransaction } from "@stellar/freighter-api";

export const MILESTONE_CONTRACT_ID = process.env.NEXT_PUBLIC_ORBIT_CONTRACT_ID || "";
export const MILESTONE_TOKEN_ID = process.env.NEXT_PUBLIC_USDC_CONTRACT_ID || "";
export const MILESTONE_RPC = process.env.NEXT_PUBLIC_SOROBAN_RPC_URL || "https://soroban-testnet.stellar.org";
export const MILESTONE_NETWORK = process.env.NEXT_PUBLIC_STELLAR_NETWORK_PASSPHRASE || Networks.TESTNET;

export type MilestoneStageDraft = {
  title: string;
  deliverableUrl: string;
  amount: string;
  dueDate: string;
};

export type LiveMilestoneStage = {
  title: string;
  deliverableUrl: string;
  amount: string;
  dueTimestamp: number;
  proofUrl: string;
  reviewFeedback: string;
  submitted: boolean;
  released: boolean;
};

export type LiveMilestoneProject = {
  id: string;
  merchant: string;
  contractor: string;
  escrowed: string;
  releasedAmount: string;
  stages: LiveMilestoneStage[];
};

export type MilestoneReceipt = { hash: string; ledger: number; explorerUrl: string; result?: unknown };

const UNITS = 10_000_000n;

export function toMilestoneUnits(amount: string): bigint {
  const value = amount.trim();
  if (!/^(?:0|[1-9]\d*)(?:\.\d{1,7})?$/.test(value)) throw new Error("Use a positive USDC amount with at most seven decimal places.");
  const [integer, fractional = ""] = value.split(".");
  const units = BigInt(integer) * UNITS + BigInt(fractional.padEnd(7, "0"));
  if (units <= 0n || units > (2n ** 127n - 1n)) throw new Error("Milestone amount is out of range.");
  return units;
}

export function formatMilestoneUnits(value: bigint | number | string): string {
  const amount = BigInt(value);
  const sign = amount < 0n ? "-" : "";
  const abs = amount < 0n ? -amount : amount;
  const fraction = (abs % UNITS).toString().padStart(7, "0").replace(/0+$/, "");
  return sign + (abs / UNITS).toString() + (fraction ? "." + fraction : "");
}

export function validateMilestoneStages(stages: MilestoneStageDraft[]): bigint {
  if (!Array.isArray(stages) || stages.length < 1 || stages.length > 12) {
    throw new Error("Provide 1–12 milestones.");
  }
  let total = 0n;
  for (const [index, stage] of stages.entries()) {
    if (!stage.title.trim() || stage.title.trim().length > 120) throw new Error(`Milestone ${index + 1}: title is required (max 120 characters).`);
    if (stage.deliverableUrl.length > 512) throw new Error(`Milestone ${index + 1}: deliverable URL exceeds 512 characters.`);
    if (stage.deliverableUrl && !/^https:\/\//i.test(stage.deliverableUrl)) throw new Error(`Milestone ${index + 1}: use an HTTPS deliverable URL.`);
    const deadline = Date.parse(stage.dueDate);
    if (!Number.isFinite(deadline) || deadline <= Date.now()) throw new Error(`Milestone ${index + 1}: enter a future due date.`);
    total += toMilestoneUnits(stage.amount);
  }
  if (total > 2n ** 127n - 1n) throw new Error("Escrow total exceeds the Soroban token range.");
  return total;
}

export function validateMilestoneLink(url: string): string {
  const value = url.trim();
  if (value.length < 1 || value.length > 512 || !/^https:\/\//i.test(value)) {
    throw new Error("Supply a secure HTTPS URL (max 512 characters).");
  }
  return value;
}

export function requireMilestoneContract(): void {
  if (!MILESTONE_CONTRACT_ID || !MILESTONE_TOKEN_ID) {
    throw new Error("Milestone escrow contract and token IDs must be configured in NEXT_PUBLIC_ORBIT_CONTRACT_ID and NEXT_PUBLIC_USDC_CONTRACT_ID.");
  }
}

const symbol = (name: string): xdr.ScVal => xdr.ScVal.scvSymbol(name);
const stringVal = (value: string): xdr.ScVal => nativeToScVal(value, { type: "string" });
const u64 = (value: string | number | bigint): xdr.ScVal => nativeToScVal(BigInt(value), { type: "u64" });

export function encodeMilestoneStages(stages: MilestoneStageDraft[]): xdr.ScVal {
  validateMilestoneStages(stages);
  return xdr.ScVal.scvVec(stages.map((stage) => xdr.ScVal.scvMap([
    new xdr.ScMapEntry({ key: symbol("amount"), val: nativeToScVal(toMilestoneUnits(stage.amount), { type: "i128" }) }),
    new xdr.ScMapEntry({ key: symbol("deliverable_url"), val: stringVal(stage.deliverableUrl) }),
    new xdr.ScMapEntry({ key: symbol("due_timestamp"), val: u64(Math.floor(Date.parse(stage.dueDate) / 1000)) }),
    new xdr.ScMapEntry({ key: symbol("title"), val: stringVal(stage.title.trim()) }),
  ]))));
}

async function connectedFreighter(): Promise<string> {
  const state = await isConnected();
  if (state.error || !state.isConnected) throw new Error("Connect and unlock Freighter first.");
  const access = await requestAccess();
  const address = access.address || (await getAddress()).address;
  if (!address || !StrKey.isValidEd25519PublicKey(address)) throw new Error("Freighter did not return a valid wallet address.");
  return address;
}

export async function getMilestoneWallet(): Promise<string> {
  return connectedFreighter();
}

async function invoke(method: string, args: xdr.ScVal[], address: string): Promise<MilestoneReceipt> {
  requireMilestoneContract();
  const server = new rpc.Server(MILESTONE_RPC);
  const account = await server.getAccount(address);
  const tx = new TransactionBuilder(account, { fee: BASE_FEE, networkPassphrase: MILESTONE_NETWORK })
    .addOperation(new Contract(MILESTONE_CONTRACT_ID).call(method, ...args))
    .setTimeout(90).build();
  const prepared = await server.prepareTransaction(tx);
  const signature = await signTransaction(prepared.toXDR(), { address, networkPassphrase: MILESTONE_NETWORK });
  if (!signature.signedTxXdr) throw new Error(signature.error?.message || "The transaction was not signed.");
  const signedTx = TransactionBuilder.fromXDR(signature.signedTxXdr, MILESTONE_NETWORK);
  const submitted = await server.sendTransaction(signedTx);
  if (!["PENDING", "SUCCESS"].includes(submitted.status)) throw new Error("Soroban rejected this transaction: " + submitted.status);
  for (let attempt = 0; attempt < 35; attempt++) {
    const result = await server.getTransaction(submitted.hash);
    if (result.status === "SUCCESS") {
      return {
        hash: submitted.hash,
        ledger: result.ledger || 0,
        explorerUrl: `https://stellar.expert/explorer/testnet/tx/${submitted.hash}`,
        result: result.returnValue ? scValToNative(result.returnValue) : undefined,
      };
    }
    if (result.status === "FAILED") throw new Error("The Soroban transaction failed. No milestone release was confirmed.");
    await new Promise((resolve) => setTimeout(resolve, 1500));
  }
  throw new Error(`Transaction ${submitted.hash} was submitted but is not yet confirmed; look it up before retrying.`);
}

export async function createMilestoneEscrow(contractor: string, stages: MilestoneStageDraft[]): Promise<MilestoneReceipt> {
  if (!StrKey.isValidEd25519PublicKey(contractor)) throw new Error("Contractor must be a valid Stellar G... address.");
  validateMilestoneStages(stages);
  const merchant = await connectedFreighter();
  if (merchant === contractor) throw new Error("Contractor and merchant must be different wallets.");
  return invoke("create_milestone_project", [
    Address.fromString(merchant).toScVal(), Address.fromString(contractor).toScVal(),
    Address.fromString(MILESTONE_TOKEN_ID).toScVal(), encodeMilestoneStages(stages),
  ], merchant);
}

export async function submitMilestoneProof(id: string, stageIndex: number, proofUrl: string): Promise<MilestoneReceipt> {
  const contractor = await connectedFreighter();
  return invoke("submit_milestone_deliverable", [
    Address.fromString(contractor).toScVal(), u64(id), nativeToScVal(stageIndex, { type: "u32" }),
    stringVal(validateMilestoneLink(proofUrl)),
  ], contractor);
}

export async function requestMilestoneChanges(id: string, stageIndex: number, feedback: string): Promise<MilestoneReceipt> {
  const merchant = await connectedFreighter();
  if (!feedback.trim() || feedback.length > 512) throw new Error("Feedback is required (max 512 characters).");
  return invoke("request_milestone_changes", [
    Address.fromString(merchant).toScVal(), u64(id), nativeToScVal(stageIndex, { type: "u32" }),
    stringVal(feedback.trim()),
  ], merchant);
}

export async function releaseMilestoneEscrow(id: string, stageIndex: number): Promise<MilestoneReceipt> {
  const merchant = await connectedFreighter();
  return invoke("approve_and_release_milestone", [
    Address.fromString(merchant).toScVal(), u64(id), nativeToScVal(stageIndex, { type: "u32" }),
  ], merchant);
}

export async function readMilestoneEscrow(id: string): Promise<LiveMilestoneProject | null> {
  requireMilestoneContract();
  if (!/^\d+$/.test(id)) throw new Error("Enter a numeric on-chain project ID.");
  const server = new rpc.Server(MILESTONE_RPC);
  // A simulated read is safe: no signature and no transaction is sent.
  const account = await server.getAccount(await connectedFreighter());
  const tx = new TransactionBuilder(account, { fee: BASE_FEE, networkPassphrase: MILESTONE_NETWORK })
    .addOperation(new Contract(MILESTONE_CONTRACT_ID).call("get_milestone_project", u64(id)))
    .setTimeout(30).build();
  const simulated = await server.simulateTransaction(tx);
  if (!simulated.result?.retval) throw new Error(simulated.error || "Could not read this project on-chain.");
  const result = scValToNative(simulated.result.retval) as Record<string, unknown> | null;
  if (!result) return null;
  const stages = (result.stages as Array<Record<string, unknown>> || []).map((stage) => ({
    title: String(stage.title ?? ""),
    deliverableUrl: String(stage.deliverable_url ?? ""),
    amount: formatMilestoneUnits(stage.amount as string),
    dueTimestamp: Number(stage.due_timestamp),
    proofUrl: String(stage.proof_url ?? ""),
    reviewFeedback: String(stage.review_feedback ?? ""),
    submitted: Boolean(stage.submitted),
    released: Boolean(stage.released),
  }));
  return {
    id: String(result.id),
    merchant: String(result.merchant),
    contractor: String(result.contractor),
    escrowed: formatMilestoneUnits(result.escrowed as string),
    releasedAmount: formatMilestoneUnits(result.released_amount as string),
    stages,
  };
}
