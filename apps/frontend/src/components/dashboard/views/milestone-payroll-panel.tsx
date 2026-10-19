"use client";

import { useCallback, useEffect, useState } from "react";
import { CheckCircle2, ExternalLink, FileCheck2, Plus, RefreshCw, ShieldCheck, Trash2 } from "lucide-react";
import {
  createMilestoneEscrow, getMilestoneWallet, readMilestoneEscrow,
  releaseMilestoneEscrow, requestMilestoneChanges, submitMilestoneProof,
  validateMilestoneStages,
  type LiveMilestoneProject, type MilestoneStageDraft, type MilestoneReceipt,
} from "@/lib/milestone-escrow";

const newStage = (): MilestoneStageDraft => ({ title: "", deliverableUrl: "", amount: "", dueDate: "" });

export function MilestonePayrollPanel({ canRunPayroll }: { canRunPayroll: boolean }) {
  const [contractor, setContractor] = useState("");
  const [stages, setStages] = useState<MilestoneStageDraft[]>([newStage()]);
  const [projectId, setProjectId] = useState("");
  const [project, setProject] = useState<LiveMilestoneProject | null>(null);
  const [wallet, setWallet] = useState("");
  const [proofInput, setProofInput] = useState<Record<number, string>>({});
  const [feedbackInput, setFeedbackInput] = useState<Record<number, string>>({});
  const [receipt, setReceipt] = useState<MilestoneReceipt | null>(null);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [working, setWorking] = useState(false);
  const [view, setView] = useState<"create" | "tracker">("tracker");
  const isMerchant = Boolean(project && wallet && wallet === project.merchant);
  const isContractor = Boolean(project && wallet && wallet === project.contractor);

  const refresh = useCallback(async (id = projectId) => {
    if (!id.trim()) return;
    const current = await readMilestoneEscrow(id.trim());
    if (!current) throw new Error("No Soroban milestone project with this ID exists.");
    setProject(current);
    setNotice("Escrow state loaded from the Stellar ledger.");
  }, [projectId]);

  useEffect(() => {
    if (!projectId || !wallet || view !== "tracker") return;
    let active = true;
    const timer = setInterval(() => {
      readMilestoneEscrow(projectId).then((next) => {
        if (active && next) setProject(next);
      }).catch(() => {});
    }, 15000);
    return () => { active = false; clearInterval(timer); };
  }, [projectId, wallet, view]);

  async function perform(action: () => Promise<MilestoneReceipt>, message: string) {
    setWorking(true); setError(""); setNotice("");
    try {
      const result = await action();
      setReceipt(result);
      setNotice(message);
      if (projectId) await refresh(projectId);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : "Could not confirm the transaction.");
    } finally {
      setWorking(false);
    }
  }

  async function connect() {
    try {
      setError("");
      setWallet(await getMilestoneWallet());
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : "Could not connect wallet.");
    }
  }

  async function build() {
    if (!canRunPayroll) { setError("Your team role cannot create payroll agreements."); return; }
    try { validateMilestoneStages(stages); }
    catch (reason) { setError(reason instanceof Error ? reason.message : "Invalid milestones."); return; }
    setWorking(true); setError(""); setNotice("");
    try {
      const result = await createMilestoneEscrow(contractor.trim(), stages);
      setReceipt(result);
      const id = result.result === undefined || result.result === null ? "" : String(result.result);
      if (id) {
        setProjectId(id);
        setView("tracker");
        await refresh(id);
      }
      setNotice("Milestone project funded on-chain. Share the project ID with your contractor.");
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : "Escrow creation failed.");
    } finally { setWorking(false); }
  }

  const changeStage = (index: number, key: keyof MilestoneStageDraft, value: string) =>
    setStages((current) => current.map((item, i) => i === index ? { ...item, [key]: value } : item));

  return (
    <section className="mb-6 rounded-2xl border border-black/15 bg-white p-5 shadow-sm" aria-label="Milestone escrow payroll">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <div className="flex items-center gap-2"><ShieldCheck size={18} /><h3 className="font-bold text-lg">Milestone escrow payroll</h3></div>
          <p className="mt-1 text-sm text-neutral-600">Fund every stage in Soroban escrow. Contractors submit proof; only the merchant can sign a token release.</p>
        </div>
        <button type="button" disabled={working} onClick={connect} className="button button-secondary px-3 py-2 text-xs">
          {wallet ? `Wallet ${wallet.slice(0, 6)}…${wallet.slice(-4)}` : "Connect Freighter"}
        </button>
      </div>
      <div className="mt-4 flex gap-2 border-b pb-3 text-sm">
        <button type="button" onClick={() => setView("tracker")} className={view === "tracker" ? "font-semibold underline" : "text-neutral-500"}>Track & review</button>
        <button type="button" onClick={() => setView("create")} className={view === "create" ? "font-semibold underline" : "text-neutral-500"}>Create agreement</button>
      </div>

      {view === "create" && (
        <div className="mt-4 space-y-4">
          <label className="block text-xs font-semibold">Contractor Stellar address (G…)
            <input className="mt-1 w-full rounded-lg border p-2 font-mono text-sm" value={contractor} onChange={(e) => setContractor(e.target.value)} placeholder="G..." />
          </label>
          <div className="space-y-3">
            {stages.map((stage, index) => (
              <div key={index} className="rounded-xl bg-neutral-50 border p-3 space-y-2">
                <div className="flex justify-between text-sm font-semibold">Stage {index + 1}
                  <button type="button" onClick={() => setStages((list) => list.filter((_, i) => i !== index))} disabled={stages.length === 1 || working} title="Remove stage"><Trash2 size={15} /></button>
                </div>
                <input aria-label={`Stage ${index + 1} title`} className="w-full border rounded p-2 text-sm" placeholder="Milestone title" maxLength={120} value={stage.title} onChange={(e) => changeStage(index, "title", e.target.value)} />
                <input aria-label={`Stage ${index + 1} deliverable URL`} className="w-full border rounded p-2 text-sm" placeholder="Deliverable specification URL (optional, HTTPS)" value={stage.deliverableUrl} onChange={(e) => changeStage(index, "deliverableUrl", e.target.value)} />
                <div className="grid grid-cols-2 gap-2">
                  <label className="text-xs">USDC amount<input className="mt-1 w-full border rounded p-2" type="text" inputMode="decimal" placeholder="100.00" value={stage.amount} onChange={(e) => changeStage(index, "amount", e.target.value)} /></label>
                  <label className="text-xs">Due date (UTC)<input className="mt-1 w-full border rounded p-2" type="date" value={stage.dueDate} onChange={(e) => changeStage(index, "dueDate", e.target.value)} /></label>
                </div>
              </div>
            ))}
          </div>
          <div className="flex flex-wrap items-center gap-3">
            <button type="button" disabled={stages.length >= 12 || working} onClick={() => setStages((items) => [...items, newStage()])} className="button button-secondary px-3 py-2 text-xs inline-flex gap-2"><Plus size={14} />Add stage</button>
            <button type="button" disabled={!canRunPayroll || working || !wallet} onClick={build} className="button button-primary px-4 py-2 text-xs">
              {working ? "Waiting for Freighter & ledger…" : "Sign & fund escrow"}
            </button>
          </div>
        </div>
      )}

      {view === "tracker" && (
        <div className="mt-4 space-y-4">
          <div className="flex gap-2">
            <input aria-label="On-chain milestone project ID" inputMode="numeric" placeholder="On-chain project ID" className="min-w-0 flex-1 rounded-lg border px-3 py-2 font-mono text-sm" value={projectId} onChange={(e) => setProjectId(e.target.value)} />
            <button type="button" disabled={working || !wallet || !projectId} onClick={() => { setError(""); refresh().catch((e: Error) => setError(e.message)); }} className="button button-secondary px-3 py-2 text-xs inline-flex items-center gap-1"><RefreshCw size={14} />Load</button>
          </div>
          {project && (
            <div className="space-y-3">
              <div className="rounded-xl bg-neutral-50 border p-3 text-sm">
                <p><strong>Escrow #{project.id}</strong> • {project.releasedAmount} / {project.escrowed} USDC released</p>
                <p className="font-mono text-xs mt-1 break-all">Merchant: {project.merchant}</p>
                <p className="font-mono text-xs mt-1 break-all">Contractor: {project.contractor}</p>
                <p className="mt-1 text-xs text-neutral-600">{isMerchant ? "Merchant review access" : isContractor ? "Contractor submission access" : "Read-only access"}</p>
              </div>
              {project.stages.map((stage, index) => (
                <article key={index} className="rounded-xl border p-4 space-y-2 text-sm">
                  <div className="flex flex-wrap justify-between gap-2">
                    <h4 className="font-semibold">{index + 1}. {stage.title}</h4>
                    <span className={stage.released ? "text-emerald-700" : "text-amber-700"}>{stage.released ? "Released" : stage.submitted ? "Awaiting approval" : "Awaiting deliverable"}</span>
                  </div>
                  <p>{stage.amount} USDC · due {new Date(stage.dueTimestamp * 1000).toLocaleDateString()}</p>
                  {stage.deliverableUrl && <a href={stage.deliverableUrl} target="_blank" rel="noopener noreferrer" className="text-blue-700 underline">Original deliverable specification</a>}
                  {stage.proofUrl && <p>Proof: <a href={stage.proofUrl} target="_blank" rel="noopener noreferrer" className="text-blue-700 underline break-all">{stage.proofUrl}</a></p>}
                  {stage.reviewFeedback && <p className="text-amber-700">Requested changes: {stage.reviewFeedback}</p>}
                  {isContractor && !stage.released && (
                    <div className="flex gap-2">
                      <input aria-label={`Stage ${index + 1} proof URL`} className="min-w-0 flex-1 border rounded p-2" placeholder="HTTPS link to completed work" value={proofInput[index] || ""} onChange={(e) => setProofInput((values) => ({ ...values, [index]: e.target.value }))} />
                      <button type="button" disabled={working} className="button button-primary px-3 text-xs" onClick={() => perform(() => submitMilestoneProof(project.id, index, proofInput[index] || ""), "Proof submitted on-chain for merchant review.")}>Submit</button>
                    </div>
                  )}
                  {isMerchant && stage.submitted && !stage.released && (
                    <div className="space-y-2">
                      <input aria-label={`Stage ${index + 1} feedback`} className="w-full border rounded p-2" placeholder="Feedback if revisions are required" maxLength={512} value={feedbackInput[index] || ""} onChange={(e) => setFeedbackInput((values) => ({ ...values, [index]: e.target.value }))} />
                      <div className="flex gap-2">
                        <button type="button" disabled={working} className="button button-secondary px-3 py-2 text-xs" onClick={() => perform(() => requestMilestoneChanges(project.id, index, feedbackInput[index] || ""), "Revisions requested. Funds remain locked.")}>Request changes</button>
                        <button type="button" disabled={working || !canRunPayroll} className="button button-primary px-3 py-2 text-xs inline-flex gap-1 items-center" onClick={() => perform(() => releaseMilestoneEscrow(project.id, index), "Contractor payment confirmed on-chain.")}><FileCheck2 size={14} />Approve & release</button>
                      </div>
                    </div>
                  )}
                  {stage.released && <p className="flex items-center gap-1 text-emerald-700 text-xs"><CheckCircle2 size={14} />Token release confirmed by Soroban state</p>}
                </article>
              ))}
            </div>
          )}
        </div>
      )}
      {error && <p role="alert" className="mt-4 text-sm text-red-700 break-words">{error}</p>}
      {notice && <p role="status" className="mt-4 text-sm text-emerald-800">{notice}</p>}
      {receipt && (
        <a href={receipt.explorerUrl} target="_blank" rel="noopener noreferrer" className="mt-3 inline-flex items-center gap-1 text-xs underline text-blue-700">
          <ExternalLink size={12} />View signed transaction {receipt.hash.slice(0, 12)}… at ledger {receipt.ledger}
        </a>
      )}
      <p className="mt-4 text-xs text-neutral-500">
        On-chain operations require the latest deployed Orbit Soroban contract with milestone entrypoints. Browser state is not used as proof of payment.
      </p>
    </section>
  );
}
