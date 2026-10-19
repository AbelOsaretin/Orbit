"use client";

import { PriceDisplay } from "@/components/ui/price-display";
import { useState, useRef, useMemo, useEffect, useDeferredValue } from "react";
import {
  SendHorizontal,
  Upload,
  Download,
  Plus,
  Trash2,
  CheckCircle2,
  AlertCircle,
  RotateCw,
  Zap,
  ExternalLink,
  Copy,
  Check,
  FileSpreadsheet,
  ShieldCheck,
  X,
  FlaskConical,
} from "lucide-react";
import { useTranslations } from "next-intl";
import Papa from "papaparse";
import { StrKey } from "@stellar/stellar-sdk";
import { PayrollRecipient, PayrollSimulationResult } from "../dashboard-types";
import { useTeam } from "@/lib/team-context";
import { useAuth } from "@/lib/auth-context";
import { Contractor, duplicateWallets, mergeDuplicateRecipients, filterAndSortRecipients, parsePayrollAmount } from "@/lib/payroll";
import { simulatePayrollRun } from "@/lib/payroll-simulation";
import { PayrollSimulationModal } from "../modals/payroll-simulation-modal";
import { MilestonePayrollPanel } from "./milestone-payroll-panel";

const SAMPLE_RECIPIENTS: PayrollRecipient[] = [
  {
    id: "rec_charlie_01",
    name: "Charlie Vance",
    role: "Smart Contract Lead",
    walletAddress: "GD3AGJRMLRKRTQKBVMLT6AW34IZZ3IEALRHTY247BBNISVS452SMQDPG",
    amount: 500.0,
    network: "stellar-testnet",
    status: "ready",
  },
  {
    id: "rec_sarah_02",
    name: "Sarah Lin",
    role: "Frontend Engineer",
    walletAddress: "GBXCR3C3CEKQ3UYNA3UUN2BUBB3GOGWN4VPR2FJ2G2WFQJLFVKJXDJ3A",
    amount: 400.0,
    network: "stellar-testnet",
    status: "ready",
  },
  {
    id: "rec_david_03",
    name: "David Kim",
    role: "UI/UX Designer",
    walletAddress: "GAJ7J66Q66THHJ22WTBBEJUHHLVD7RZZW6KTVQDBBQYXRPVC4UFFQFDR",
    amount: 200.0,
    network: "stellar-testnet",
    status: "ready",
  },
  {
    id: "rec_elena_04",
    name: "Elena Rostova",
    role: "Community Manager",
    walletAddress: "GBDXVD27PJS4PVC5USFBSJUTTYVXIH6MS6MM6M2IDCXJWJHEPRTTIQNP",
    amount: 100.0,
    network: "stellar-testnet",
    status: "ready",
  },
];

interface RowValidationError {
  row: number;
  reason: string;
}

export function PayrollView() {
  const t = useTranslations("Tables");
  const { can } = useTeam();
  const canRunPayroll = can("runPayroll");
  const { user } = useAuth();
  const addressBookKey = user?.id ? `orbit_contractors:${user.id}` : null;
  const [contractors, setContractors] = useState<Contractor[]>([]);
  const [bookError, setBookError] = useState("");
  const [showAddressBook, setShowAddressBook] = useState(false);
  const [search, setSearch] = useState("");
  const deferredSearch = useDeferredValue(search);
  const [sortColumn, setSortColumn] = useState<"name" | "amount">("name");
  const [sortDirection, setSortDirection] = useState<"asc" | "desc">("asc");
  const [recipients, setRecipients] = useState<PayrollRecipient[]>(SAMPLE_RECIPIENTS);
  const [isExecuting, setIsExecuting] = useState(false);
  const [disbursedSuccess, setDisbursedSuccess] = useState<string | null>(null);
  const [isSimulating, setIsSimulating] = useState(false);
  const [isSimulationOpen, setIsSimulationOpen] = useState(false);
  const [simulationResult, setSimulationResult] = useState<PayrollSimulationResult | null>(null);
  const [uploadErrors, setUploadErrors] = useState<RowValidationError[]>([]);
  const [copiedKey, setCopiedKey] = useState<string | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const duplicates = useMemo(() => duplicateWallets(recipients), [recipients]);
  const visibleRecipients = useMemo(() => filterAndSortRecipients(recipients, deferredSearch, sortColumn, sortDirection),
    [recipients, deferredSearch, sortColumn, sortDirection]);

  useEffect(() => {
    setContractors([]); setBookError("");
    if (!addressBookKey) return;
    try {
      const stored = JSON.parse(localStorage.getItem(addressBookKey) || "[]");
      if (!Array.isArray(stored)) throw new Error("Invalid address book");
      const verified = new Map<string, Contractor>();
      for (const row of stored) if (typeof row.name === "string" && typeof row.role === "string" &&
        typeof row.walletAddress === "string" && StrKey.isValidEd25519PublicKey(row.walletAddress) &&
        typeof row.amount === "number" && Number.isFinite(row.amount) && parsePayrollAmount(row.amount.toFixed(7)) !== null) {
        verified.set(row.walletAddress, row);
      }
      setContractors([...verified.values()]);
    } catch { setBookError("The saved address book could not be read."); }
  }, [addressBookKey]);

  const saveContractor = (recipient: PayrollRecipient) => {
    if (!addressBookKey || !StrKey.isValidEd25519PublicKey(recipient.walletAddress)) {
      setBookError("Sign in and use a valid Stellar address before saving a contractor."); return;
    }
    const entry: Contractor = { name: recipient.name, role: recipient.role, walletAddress: recipient.walletAddress, amount: recipient.amount };
    const next = [...contractors.filter((row) => row.walletAddress !== entry.walletAddress), entry];
    try { localStorage.setItem(addressBookKey, JSON.stringify(next)); setContractors(next); setBookError(""); }
    catch { setBookError("Saving failed. Check that browser storage is available."); }
  };

  const sortBy = (column: "name" | "amount") => {
    if (sortColumn === column) setSortDirection((previous) => previous === "asc" ? "desc" : "asc");
    else { setSortColumn(column); setSortDirection("asc"); }
  };

  // Manual Add Modal / Row state
  const [isAddingManual, setIsAddingManual] = useState(false);
  const [newName, setNewName] = useState("");
  const [newRole, setNewRole] = useState("");
  const [newWallet, setNewWallet] = useState("");
  const [newAmount, setNewAmount] = useState("");

  const copyToClipboard = (text: string, key: string) => {
    navigator.clipboard.writeText(text);
    setCopiedKey(key);
    setTimeout(() => setCopiedKey(null), 1800);
  };

  const totalPayout = recipients.reduce((acc, r) => acc + (r.status === "ready" ? r.amount : 0), 0);
  const readyCount = recipients.filter((r) => r.status === "ready").length;

  const handleLoadSampleCSV = () => {
    setRecipients(SAMPLE_RECIPIENTS);
    setDisbursedSuccess(null);
    setUploadErrors([]);
  };

  const handleDownloadTemplate = () => {
    const csvContent =
      "data:text/csv;charset=utf-8," +
      "Name,Role,StellarWallet,AmountUSDC\n" +
      "Charlie Vance,Smart Contract Lead,GD3AGJRMLRKRTQKBVMLT6AW34IZZ3IEALRHTY247BBNISVS452SMQDPG,500\n" +
      "Sarah Lin,Frontend Engineer,GBXCR3C3CEKQ3UYNA3UUN2BUBB3GOGWN4VPR2FJ2G2WFQJLFVKJXDJ3A,400\n" +
      "David Kim,UI/UX Designer,GAJ7J66Q66THHJ22WTBBEJUHHLVD7RZZW6KTVQDBBQYXRPVC4UFFQFDR,200\n";
    const encodedUri = encodeURI(csvContent);
    const link = document.createElement("a");
    link.setAttribute("href", encodedUri);
    link.setAttribute("download", "orbit_payroll_template.csv");
    document.body.appendChild(link);
    link.click();
    document.body.removeChild(link);
  };

  const handleFileUpload = (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (!file) return;

    Papa.parse<string[]>(file, {
      skipEmptyLines: "greedy",
      complete: (results) => {
        const rows = results.data;
        if (!rows || rows.length === 0) {
          setUploadErrors([{ row: 0, reason: "The uploaded CSV file is empty" }]);
          return;
        }

        const errors: RowValidationError[] = [];
        const validRecipients: PayrollRecipient[] = [];

        // Check if the first row is a header row
        // A header row contains header keywords and does not have a valid Stellar Ed25519 key
        const firstRowText = rows[0]?.join(" ").toLowerCase() || "";
        const firstRowCol1 = rows[0]?.[1]?.trim() || "";
        const firstRowCol2 = rows[0]?.[2]?.trim() || "";
        const firstRowHasWallet =
          StrKey.isValidEd25519PublicKey(firstRowCol1) ||
          StrKey.isValidEd25519PublicKey(firstRowCol2);

        const hasHeader =
          !firstRowHasWallet &&
          (firstRowText.includes("name") ||
            firstRowText.includes("role") ||
            firstRowText.includes("wallet") ||
            firstRowText.includes("address") ||
            firstRowText.includes("amount"));

        const startIdx = hasHeader ? 1 : 0;

        for (let i = startIdx; i < rows.length; i++) {
          const rowNum = i + 1; // 1-indexed row number matching file line
          const parts = rows[i].map((p) => (typeof p === "string" ? p.trim() : ""));

          // Skip completely empty rows
          if (parts.length === 0 || parts.every((p) => p.length === 0)) {
            continue;
          }

          if (parts.length < 3) {
            errors.push({
              row: rowNum,
              reason: `Insufficient columns (${parts.length} found, expected at least 3: Name, [Role], StellarWallet, AmountUSDC)`,
            });
            continue;
          }

          const name = parts[0] || `Recipient ${rowNum}`;
          const role = parts.length >= 4 ? parts[1] || "Contractor" : "Contractor";
          const wallet = parts.length >= 4 ? parts[2] : parts[1];
          const rawAmount = parts.length >= 4 ? parts[3] : parts[2];

          const rowReasons: string[] = [];

          // Validate wallet address with StrKey.isValidEd25519PublicKey
          if (!wallet) {
            rowReasons.push("Missing wallet address");
          } else if (!StrKey.isValidEd25519PublicKey(wallet)) {
            rowReasons.push(`Invalid Stellar wallet address "${wallet}"`);
          }

          // Validate amount: reject missing, zero, or negative amounts
          if (!rawAmount || rawAmount === "") {
            rowReasons.push("Missing amount");
          } else {
            if (parsePayrollAmount(rawAmount) === null) {
              rowReasons.push(`Amount must be positive, with at most 7 decimal places and within the supported range (got "${rawAmount}")`);
            }
          }

          if (rowReasons.length > 0) {
            errors.push({
              row: rowNum,
              reason: rowReasons.join(", "),
            });
          } else {
            validRecipients.push({
              id: `rec_csv_${rowNum}_${Math.random().toString(36).substring(2, 6)}`,
              name,
              role,
              walletAddress: wallet,
              amount: parsePayrollAmount(rawAmount)!,
              network: "stellar-testnet",
              status: "ready",
            });
          }
        }

        setUploadErrors(errors);

        setRecipients(validRecipients);
        setDisbursedSuccess(null);
      },
      error: (error) => {
        setUploadErrors([{ row: 0, reason: `Failed to parse CSV: ${error.message}` }]);
      },
    });

    e.target.value = "";
  };

  const handleAddManual = (e: React.FormEvent) => {
    e.preventDefault();
    if (!newName.trim() || !newWallet.trim() || !newAmount) return;

    if (!StrKey.isValidEd25519PublicKey(newWallet.trim())) {
      setUploadErrors([
        {
          row: 0,
          reason: `Invalid Stellar address "${newWallet.trim()}". Must be a valid 56-character Ed25519 public key starting with G.`,
        },
      ]);
      return;
    }

    const parsedAmount = parsePayrollAmount(newAmount);
    if (parsedAmount === null) {
      setUploadErrors([
        {
          row: 0,
          reason: `Invalid amount "${newAmount}". Amount must be greater than zero.`,
        },
      ]);
      return;
    }

    const newRec: PayrollRecipient = {
      id: `rec_man_${Math.random().toString(36).substring(2, 8)}`,
      name: newName.trim(),
      role: newRole.trim() || "Contractor",
      walletAddress: newWallet.trim(),
      amount: parsedAmount,
      network: "stellar-testnet",
      status: "ready",
    };

    setRecipients((prev) => [...prev, newRec]);
    setNewName("");
    setNewRole("");
    setNewWallet("");
    setNewAmount("");
    setIsAddingManual(false);
    setUploadErrors([]);
  };

  const handleRemove = (id: string) => {
    setRecipients((prev) => prev.filter((r) => r.id !== id));
  };

  // Run Batch Payroll Trigger (simulates on-chain Soroban batch_disburse execution)
  const handleRunBatchPayroll = () => {
    if (readyCount === 0 || !canRunPayroll || duplicates.size > 0) return;
    setIsExecuting(true);
    setDisbursedSuccess(null);

    // Simulate single ledger transaction on Stellar Soroban
    setTimeout(() => {
      setRecipients((prev) =>
        prev.map((r) => ({
          ...r,
          status: "disbursed",
        }))
      );
      setIsExecuting(false);
      setDisbursedSuccess(
        `Batch Disbursement Succeeded! ${totalPayout.toLocaleString("en-US", {
          minimumFractionDigits: 2,
        })} USDC atomically distributed to ${readyCount} contractor wallets in a single ledger block (1.3s finality).`
      );
    }, 1300);
  };

  // #147 — Read-only pre-flight: build the batch_disburse envelope, simulate it
  // on Soroban RPC and surface fees, instruction limits and per-recipient checks
  // without ever prompting for a wallet signature.
  const handleSimulatePayroll = async () => {
    if (readyCount === 0) return;
    setIsSimulationOpen(true);
    setIsSimulating(true);
    setSimulationResult(null);
    const result = await simulatePayrollRun({
      recipients,
      sender: user?.walletAddress || user?.linkedVaults?.stellar,
    });
    setSimulationResult(result);
    setIsSimulating(false);
  };

  return (
    <div className="space-y-6">
      <MilestonePayrollPanel canRunPayroll={canRunPayroll} />
      {/* 1. Header with Actions */}
      <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-4 pb-2">
        <div>
          <div className="flex items-center gap-2">
            <h2 className="text-xl font-bold tracking-tight text-black">
              Batch Payroll Engine
            </h2>
            <span className="text-[10px] font-mono px-2 py-0.5 rounded-full bg-black text-white font-bold tracking-wide">
              SUPERPOWER
            </span>
          </div>
          <p className="text-xs text-neutral-500 mt-0.5">
            Disburse global contractor payroll in USDC with a single signature on Stellar Soroban.
          </p>
        </div>

        <div className="flex items-center gap-2 self-start sm:self-auto flex-wrap">
          <input
            type="file"
            accept=".csv"
            ref={fileInputRef}
            onChange={handleFileUpload}
            className="hidden"
          />
          <button
            onClick={() => fileInputRef.current?.click()}
            className="button button-secondary text-xs h-9 px-3 flex items-center gap-1.5"
          >
            <Upload size={13} />
            <span>Upload CSV</span>
          </button>
          <button
            onClick={handleDownloadTemplate}
            className="button button-secondary text-xs h-9 px-3 flex items-center gap-1.5"
            title="Download blank sample CSV"
          >
            <Download size={13} />
            <span>Template</span>
          </button>
          <button
            onClick={() => setIsAddingManual(true)}
            className="button button-secondary text-xs h-9 px-3 flex items-center gap-1.5"
          >
            <Plus size={13} />
            <span>Add Row</span>
          </button>
          <button
            onClick={handleSimulatePayroll}
            disabled={isSimulating || isExecuting || readyCount === 0}
            title="Simulate the batch against Soroban RPC without signing"
            className="button button-secondary text-xs h-9 px-3 flex items-center gap-1.5 disabled:opacity-50"
          >
            {isSimulating ? <RotateCw size={13} className="animate-spin" /> : <FlaskConical size={13} />}
            <span>Simulate Payroll Run</span>
          </button>
          <button
            onClick={handleLoadSampleCSV}
            className="button button-secondary text-xs h-9 px-3 text-neutral-600 hover:text-black"
          >
            Reset Demo
          </button>
        </div>
      </div>

      {/* Success Notification Banner */}
      {disbursedSuccess && (
        <div className="p-4 rounded-xl bg-emerald-50 border border-emerald-200 text-xs text-emerald-950 flex items-start gap-3 animate-in fade-in duration-200 shadow-xs">
          <CheckCircle2 size={18} className="text-emerald-600 shrink-0 mt-0.5" />
          <div className="flex-1">
            <span className="font-semibold">Atomic Ledger Execution Confirmed:</span>{" "}
            {disbursedSuccess}
          </div>
        </div>
      )}

      {/* CSV Validation Error Banner */}
      {duplicates.size > 0 && <div role="alert" className="rounded-xl border border-amber-300 bg-amber-50 p-4 text-sm">
        <p>{duplicates.size} duplicate recipient {duplicates.size === 1 ? "wallet" : "wallets"} found. Merge amounts or remove duplicate rows before running payroll.</p>
        <button className="button button-secondary mt-3 px-3 py-2" onClick={() => {
          try { setRecipients(mergeDuplicateRecipients(recipients)); }
          catch (reason) { setUploadErrors([{ row: 0, reason: reason instanceof Error ? reason.message : "Unable to merge payouts" }]); }
        }}>Merge duplicate amounts</button></div>}
      {uploadErrors.length > 0 && (
        <div className="p-4 rounded-xl bg-red-50 border border-red-200 text-xs text-red-950 space-y-2 animate-in fade-in duration-200 shadow-xs">
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-2 font-semibold text-red-700">
              <AlertCircle size={16} className="shrink-0 text-red-600" />
              <span>
                {uploadErrors.length} {uploadErrors.length === 1 ? "row" : "rows"} rejected due to validation errors (not marked ready):
              </span>
            </div>
            <button
              onClick={() => setUploadErrors([])}
              className="p-1 rounded text-red-400 hover:text-red-700 hover:bg-red-100 transition-colors cursor-pointer"
              title="Dismiss errors"
            >
              <X size={14} />
            </button>
          </div>
          <ul className="list-disc list-inside space-y-1 pl-1 text-red-800 font-mono text-[11px] max-h-48 overflow-y-auto">
            {uploadErrors.map((err, idx) => (
              <li key={idx}>
                {err.row > 0 ? (
                  <span className="font-semibold text-red-900">Row {err.row}: </span>
                ) : null}
                <span>{err.reason}</span>
              </li>
            ))}
          </ul>
        </div>
      )}

      {/* 2. Top Summary Ribbon & Run Batch CTA */}
      <div className="grid grid-cols-1 lg:grid-cols-4 gap-4">
        <div className="p-5 rounded-xl bg-white border border-black/10 shadow-xs">
          <div className="text-[11px] font-semibold uppercase tracking-wider text-neutral-400">
            Total Payout Amount
          </div>
          <div className="text-3xl font-bold text-black mt-2 tracking-tight">
            {totalPayout.toLocaleString("en-US", { minimumFractionDigits: 2 })}{" "}
            <span className="text-sm font-semibold text-neutral-500 font-mono">USDC</span>
          </div>
          <div className="text-xs text-neutral-500 mt-1">
            Across {readyCount} contractor wallets
          </div>
        </div>

        <div className="p-5 rounded-xl bg-white border border-black/10 shadow-xs">
          <div className="text-[11px] font-semibold uppercase tracking-wider text-neutral-400">
            Ledger Execution
          </div>
          <div className="text-3xl font-bold text-black mt-2 tracking-tight">
            1 Signature
          </div>
          <div className="text-xs text-neutral-500 mt-1">
            All transfers split atomically on-chain
          </div>
        </div>

        <div className="p-5 rounded-xl bg-white border border-black/10 shadow-xs">
          <div className="text-[11px] font-semibold uppercase tracking-wider text-neutral-400">
            Network Gas Overhead
          </div>
          <div className="text-3xl font-bold text-emerald-600 mt-2 tracking-tight">
            &lt; $0.001
          </div>
          <div className="text-xs text-neutral-500 mt-1">
            Sub-cent Soroban fee for entire batch
          </div>
        </div>

        <div className="p-5 rounded-xl bg-neutral-900 text-white flex flex-col justify-between shadow-xs">
          <div>
            <div className="text-[10px] font-mono text-neutral-400 uppercase tracking-wider">
              {t("payroll.oneClick")}
            </div>
            <div className="text-sm font-bold mt-1">
              {t("payroll.readyToExecute")}
            </div>
          </div>

          <button
            onClick={handleRunBatchPayroll}
            disabled={isExecuting || readyCount === 0 || !canRunPayroll || duplicates.size > 0}
            title={canRunPayroll ? undefined : t("payroll.noPermission")}
            className="w-full mt-3 button bg-white text-black hover:bg-neutral-100 font-bold text-xs h-10 px-4 flex items-center justify-center gap-2 disabled:opacity-50 transition-all cursor-pointer shadow-xs"
          >
            {isExecuting ? (
              <>
                <RotateCw size={14} className="animate-spin text-black" />
                <span>{t("payroll.broadcasting")}</span>
              </>
            ) : (
              <>
                <Zap size={14} className="text-black" />
                <span>{t("payroll.runBatch", { count: readyCount })}</span>
              </>
            )}
          </button>
        </div>
      </div>

      {/* Manual Add Form Row */}
      {isAddingManual && (
        <form
          onSubmit={handleAddManual}
          className="p-4 rounded-xl border border-black/20 bg-neutral-50 space-y-3 animate-in fade-in duration-150"
        >
          <div className="flex items-center justify-between">
            <h4 className="text-xs font-bold text-black uppercase tracking-wider">
              Add Contractor Payout Manually
            </h4>
            <button
              type="button"
              onClick={() => setIsAddingManual(false)}
              className="text-xs text-neutral-400 hover:text-black"
            >
              Cancel
            </button>
          </div>

          <div className="grid grid-cols-1 sm:grid-cols-4 gap-3">
            <input
              type="text"
              required
              placeholder="Contractor Name"
              value={newName}
              onChange={(e) => setNewName(e.target.value)}
              className="rounded-lg border border-black/10 px-3 py-2 text-xs bg-white text-black"
            />
            <input
              type="text"
              placeholder="Role (e.g. Lead Engineer)"
              value={newRole}
              onChange={(e) => setNewRole(e.target.value)}
              className="rounded-lg border border-black/10 px-3 py-2 text-xs bg-white text-black"
            />
            <input
              type="text"
              required
              placeholder="Stellar Address (Starts with G...)"
              value={newWallet}
              onChange={(e) => setNewWallet(e.target.value)}
              className="rounded-lg border border-black/10 px-3 py-2 text-xs font-mono bg-white text-black"
            />
            <div className="flex items-center gap-2">
              <input
                type="number"
                required
                min="0.0000001"
                step="0.0000001"
                placeholder="USDC Amount"
                value={newAmount}
                onChange={(e) => setNewAmount(e.target.value)}
                className="w-full rounded-lg border border-black/10 px-3 py-2 text-xs font-mono bg-white text-black"
              />
              <button
                type="submit"
                className="button button-primary text-xs h-9 px-4 shrink-0"
              >
                Add
              </button>
            </div>
          </div>
        </form>
      )}

      {/* 3. Recipients Table */}
      <div className="rounded-xl border bg-white p-4 text-xs">
        <button className="button button-secondary px-3 py-2" onClick={() => setShowAddressBook((previous) => !previous)}>Saved contractors ({contractors.length})</button>
        {bookError && <p role="alert" className="mt-2 text-red-700">{bookError}</p>}
        {showAddressBook && <div className="mt-3 max-h-64 overflow-auto space-y-2">
          <p className="text-neutral-500">Addresses are validated as Stellar public keys. Confirm ownership with the contractor before payment.</p>
          {contractors.length === 0 && <p>No saved contractors yet. Use “Save contractor” on a payout row.</p>}
          {contractors.map((contractor) => <div key={contractor.walletAddress} className="flex flex-wrap items-center gap-3 rounded-lg border p-3">
            <div className="min-w-0 flex-1"><strong>{contractor.name}</strong><p className="break-all font-mono">{contractor.walletAddress}</p><p><PriceDisplay amount={contractor.amount} /></p></div>
            <button className="button button-secondary px-3 py-2" disabled={isExecuting} onClick={() => {
              setRecipients((previous) => [...previous, { ...contractor, id: crypto.randomUUID(), network: "stellar-testnet", status: "ready" }]); setDisbursedSuccess(null);
            }}>Quick add</button>
            <button aria-label={`Remove ${contractor.name} from address book`} onClick={() => {
              const next = contractors.filter((row) => row.walletAddress !== contractor.walletAddress);
              try { if (addressBookKey) localStorage.setItem(addressBookKey, JSON.stringify(next)); setContractors(next); }
              catch { setBookError("The contractor could not be removed from storage."); }
            }}>Remove</button>
          </div>)}
        </div>}
      </div>
      <div className="rounded-xl border border-black/10 bg-white overflow-hidden shadow-xs">
        <div className="flex flex-wrap items-center gap-3 border-b p-4 text-xs"><label htmlFor="payroll-search">Search payouts</label>
          <input id="payroll-search" type="search" value={search} onChange={(event) => setSearch(event.target.value)} placeholder="Name or wallet address"
            className="min-w-0 flex-1 rounded-lg border px-3 py-2" /><span role="status">{visibleRecipients.length} of {recipients.length} recipients</span></div>
        <div className="p-4 px-5 border-b border-black/10 flex items-center justify-between">
          <div className="flex items-center gap-2">
            <FileSpreadsheet size={16} className="text-black" />
            <h3 className="text-sm font-bold text-black">
              Payroll Distribution Manifest
            </h3>
            <span className="text-[10px] font-mono px-2 py-0.5 rounded bg-neutral-100 text-neutral-600">
              {recipients.length} recipients
            </span>
          </div>

          <div className="text-xs font-mono text-neutral-500">
            Soroban Contract: <span className="text-black font-semibold">CAZBZB...YQNG</span>
          </div>
        </div>

        {recipients.length > 0 ? (
          <div className="overflow-x-auto">
            <table className="w-full text-left text-xs">
              <thead className="bg-neutral-50/80 border-b border-black/10 text-neutral-500 uppercase tracking-wider font-semibold text-[10px]">
                <tr>
                  <th className="py-3.5 px-5" aria-sort={sortColumn === "name" ? sortDirection === "asc" ? "ascending" : "descending" : "none"}><button onClick={() => sortBy("name")}>{t("payroll.contractor")} {sortColumn === "name" ? sortDirection === "asc" ? "↑" : "↓" : "↕"}</button></th>
                  <th className="py-3.5 px-4">{t("payroll.role")}</th>
                  <th className="py-3.5 px-4">{t("payroll.address")}</th>
                  <th className="py-3.5 px-4" aria-sort={sortColumn === "amount" ? sortDirection === "asc" ? "ascending" : "descending" : "none"}><button onClick={() => sortBy("amount")}>{t("payroll.amount")} {sortColumn === "amount" ? sortDirection === "asc" ? "↑" : "↓" : "↕"}</button></th>
                  <th className="py-3.5 px-4">{t("payroll.status")}</th>
                  <th className="py-3.5 px-5 text-right">{t("payroll.action")}</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-black/5 text-neutral-700">
                {visibleRecipients.map((rec) => (
                  <tr key={rec.id} className={duplicates.has(rec.walletAddress) && rec.status === "ready" ? "bg-amber-50" : "hover:bg-neutral-50/50 transition-colors"}>
                    <td className="py-4 px-5 font-semibold text-black">
                      {rec.name}
                      {duplicates.has(rec.walletAddress) && rec.status === "ready" && <span className="ml-2 text-amber-800">Duplicate wallet</span>}
                    </td>

                    <td className="py-4 px-4 text-neutral-600">
                      {rec.role}
                    </td>

                    <td className="py-4 px-4">
                      <div className="flex items-center gap-1.5">
                        <span className="font-mono text-xs text-neutral-700">
                          {rec.walletAddress.slice(0, 6)}...{rec.walletAddress.slice(-6)}
                        </span>
                        <button
                          onClick={() => copyToClipboard(rec.walletAddress, rec.id)}
                          className="p-1 rounded hover:bg-black/5 text-neutral-400 hover:text-black transition-colors"
                          title="Copy address"
                        >
                          {copiedKey === rec.id ? (
                            <Check size={11} className="text-emerald-600" />
                          ) : (
                            <Copy size={11} />
                          )}
                        </button>
                        <a
                          href={`https://stellar.expert/explorer/testnet/account/${rec.walletAddress}`}
                          target="_blank"
                          rel="noreferrer"
                          className="p-1 rounded hover:bg-black/5 text-neutral-400 hover:text-black transition-colors"
                          title="View on Stellar Expert"
                        >
                          <ExternalLink size={11} />
                        </a>
                      </div>
                    </td>

                    <td className="py-4 px-4 font-mono font-bold text-black text-sm">
                      <PriceDisplay amount={rec.amount} />
                    </td>

                    <td className="py-4 px-4">
                      {rec.status === "disbursed" ? (
                        <span className="inline-flex items-center gap-1 text-[10px] font-mono px-2 py-0.5 rounded bg-emerald-50 text-emerald-700 border border-emerald-200 font-medium">
                          <CheckCircle2 size={11} />
                          DISBURSED
                        </span>
                      ) : (
                        <span className="inline-flex items-center gap-1 text-[10px] font-mono px-2 py-0.5 rounded bg-neutral-100 text-neutral-700 border border-black/5 font-medium">
                          READY
                        </span>
                      )}
                    </td>

                    <td className="py-4 px-5 text-right">
                      <button className="mr-2 text-xs underline" onClick={() => saveContractor(rec)}>Save contractor</button>
                      <button
                        onClick={() => handleRemove(rec.id)}
                        className="p-1.5 rounded text-neutral-400 hover:text-red-600 hover:bg-red-50 transition-colors"
                        title="Remove recipient"
                      >
                        <Trash2 size={14} />
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
            {visibleRecipients.length === 0 && <p className="p-6 text-center text-xs">No payouts match your search.</p>}
          </div>
        ) : (
          <div className="py-16 text-center">
            <div className="w-12 h-12 rounded-full bg-neutral-100 text-neutral-400 flex items-center justify-center mx-auto mb-3">
              <SendHorizontal size={22} />
            </div>
            <h4 className="font-semibold text-black text-sm">No recipients in payroll batch</h4>
            <p className="text-xs text-neutral-500 max-w-sm mx-auto mt-1 leading-relaxed">
              Upload a contractor CSV spreadsheet or click &quot;Reset Demo&quot; to test a batch payout.
            </p>
            <button
              onClick={handleLoadSampleCSV}
              className="button button-primary text-xs h-9 px-4 mt-4"
            >
              Load Sample Batch
            </button>
          </div>
        )}
      </div>

      {/* Superpower Callout */}
      <div className="p-4 rounded-xl bg-neutral-50 border border-black/5 flex items-start gap-3 text-xs text-neutral-600">
        <ShieldCheck size={18} className="text-black shrink-0 mt-0.5" />
        <div>
          <span className="font-semibold text-black">Atomic Batch Disbursement Guarantees:</span>
          {" "}Unlike traditional chains where payouts happen as N individual transactions that can partially fail, Orbit&apos;s Soroban smart contract (<code className="font-mono text-[11px] text-black">batch_disburse</code>) executes all transfers atomically. Either every single contractor gets paid, or the transaction safely rolls back.
        </div>
      </div>

      {/* Dry-run simulation inspector (#147) */}
      <PayrollSimulationModal
        isOpen={isSimulationOpen}
        isLoading={isSimulating}
        result={simulationResult}
        onClose={() => setIsSimulationOpen(false)}
      />
    </div>
  );
}
