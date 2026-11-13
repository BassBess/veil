import { useCallback, useMemo, useEffect, useEffectEvent, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./App.css";
import { rebaseDetections } from "./rebaseDetections";
import { applyReviewDecision, groupDetections, matchKey, prepareProcessing } from "./reviewActions";

type VaultStatus = { exists: boolean; unlocked: boolean; account_supported: boolean; authentication_required: boolean; code_required: boolean; needs_migration: boolean };
type DetectionRule = { phrase: string; entity_type: string };
type SavedIdentity = { id: string; entity_type: string; original: string; pseudonym: string };
type ProcessedRecord = { id: string; mode: string; text: string; count: number };
type Detection = {
  entity_type: string;
  text: string;
  start: number;
  end: number;
  confidence: number;
};

type ReviewStatus = "pending" | "accepted" | "rejected";

type ReviewedDetection = Omit<Detection, "confidence"> & {
  id: string;
  status: ReviewStatus;
  confidence: number | null;
  needsCorrection?: boolean;
};

type DetectionResponse = {
  detections: Detection[];
  suppressed?: (Detection & { reason: string })[];
};

type ValidatedDetection = Pick<
  ReviewedDetection,
  "id" | "entity_type" | "text" | "start" | "end"
>;

type ParsedDocument = {
  text: string;
  file_name: string;
  file_type: string;
  pages: number | null;
  warnings: string[];
};

function importErrorMessage(error: unknown): string {
  if (typeof error === "object" && error !== null && "message" in error && typeof error.message === "string") return error.message;
  if (typeof error === "string") return error;
  return "The document could not be imported. Please try another file.";
}

type PrivacyCategory = {
  label: string;
  value: string;
};

type ManualSelection = {
  start: number;
  end: number;
  text: string;
};

const PRIVACY_CATEGORIES: PrivacyCategory[] = [
  { label: "Person names", value: "PERSON" },
  { label: "Email addresses", value: "EMAIL_ADDRESS" },
  { label: "Phone numbers", value: "PHONE_NUMBER" },
  { label: "Locations", value: "LOCATION" },
  { label: "Organizations", value: "ORGANIZATION" },
  { label: "Street addresses", value: "STREET_ADDRESS" },
  { label: "ZIP / postal codes", value: "POSTAL_CODE" },
  { label: "Dates and times", value: "DATE_TIME" },
  { label: "Credit card numbers", value: "CREDIT_CARD" },
  { label: "IBAN bank accounts", value: "IBAN_CODE" },
  { label: "IP addresses", value: "IP_ADDRESS" },
  { label: "Website links", value: "URL" },
  { label: "Labeled ID numbers", value: "ID_NUMBER" },
  { label: "Indirect identifiers", value: "INDIRECT_IDENTIFIER" },
];

function rangesOverlap(
  first: { start: number; end: number },
  second: { start: number; end: number }
) {
  return first.start < second.end && first.end > second.start;
}

type IconName = "document" | "upload" | "home" | "database" | "vault" | "settings" | "arrow" | "plus" | "edit" | "trash";

function Icon({ name }: { name: IconName }) {
  const paths: Record<IconName, React.ReactNode> = {
    document: <><path d="M14 3H5v18h14V8Z" /><path d="M14 3v5h5M8 12h8M8 16h6" /></>,
    upload: <><path d="M12 16V3m-5 5 5-5 5 5M4 16v5h16v-5" /></>,
    home: <><path d="m3 10 9-7 9 7" /><path d="M5 9v12h5v-7h4v7h5V9" /></>,
    database: <><ellipse cx="12" cy="5" rx="8" ry="3" /><path d="M4 5v14c0 4 16 4 16 0V5M4 12c0 4 16 4 16 0" /></>,
    vault: <><rect x="3" y="3" width="18" height="18" rx="3" /><circle cx="12" cy="11" r="3" /><path d="M12 14v3" /></>,
    settings: <><path d="m9 3-1 3-3 1 1 3-2 2 2 2-1 3 3 1 1 3h6l1-3 3-1-1-3 2-2-2-2 1-3-3-1-1-3Z" /><circle cx="12" cy="12" r="3" /></>,
    arrow: <><path d="M4 12h16m-6-6 6 6-6 6" /></>,
    plus: <path d="M12 5v14M5 12h14" />,
    edit: <><path d="m15 4 5 5M4 20l5-1L21 7a2 2 0 0 0-5-5L4 14Z" /></>,
    trash: <><path d="M3 6h18M9 6V3h6v3M6 6l1 15h10l1-15M10 10v7m4-7v7" /></>,
  };
  return <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">{paths[name]}</svg>;
}

function App() {
  const [vaultProtection, setVaultProtection] = useState<"account" | "system" | "passphrase">("system");
  const [documentLanguage, setDocumentLanguage] = useState("en");
  const [suppressed, setSuppressed] = useState<(Detection & { reason: string })[]>([]);
  const [savedRules, setSavedRules] = useState<DetectionRule[]>([]);
  const [settingsMessage, setSettingsMessage] = useState("");
  const [resetError, setResetError] = useState("");
  const importInFlightRef = useRef(false);
  const [dragging, setDragging] = useState(false);
  const vaultPromptRef = useRef<HTMLDialogElement>(null);
  const resetDialogRef = useRef<HTMLDialogElement>(null);
  const resetConfirmationRef = useRef<HTMLInputElement>(null);
  const protectionPasswordRef = useRef<HTMLInputElement>(null);
  const protectionConfirmRef = useRef<HTMLInputElement>(null);
  const rulesRef = useRef<HTMLTextAreaElement>(null);
  const dropRegionRef = useRef<HTMLElement>(null);
  const [vaultStatus, setVaultStatus] = useState<VaultStatus>({ exists: false, unlocked: false, account_supported: false, authentication_required: false, code_required: false, needs_migration: false });
  const vaultLoadRef = useRef(0);
  const [vaultBusy, setVaultBusy] = useState(false);
  const [savedIdentities, setSavedIdentities] = useState<SavedIdentity[]>([]);
  const [identityQuery, setIdentityQuery] = useState("");
  const [identityCategory, setIdentityCategory] = useState("PERSON");
  const vaultPasswordRef = useRef<HTMLInputElement>(null);
  const vaultConfirmRef = useRef<HTMLInputElement>(null);
  const identityNameRef = useRef<HTMLInputElement>(null);
  const [processed, setProcessed] = useState<{record: ProcessedRecord; snapshot: string; mode: string} | null>(null);
  const [vaultRecords, setVaultRecords] = useState<ProcessedRecord[]>([]);
  const [vaultError, setVaultError] = useState("");
  const [restoredText, setRestoredText] = useState<string | null>(null);
  const [exportMessage, setExportMessage] = useState("");


  const [workspaceView, setWorkspaceView] = useState("Anonymize");
  const [reviewQuery, setReviewQuery] = useState("");
  const [reviewFilter, setReviewFilter] = useState("all");
  const [reviewCategory, setReviewCategory] = useState("all");
  const [linkRepeated, setLinkRepeated] = useState(true);
  const [reviewNotice, setReviewNotice] = useState("");
  const [activeOccurrences, setActiveOccurrences] = useState<Record<string, string>>({});
  const [reviewPage, setReviewPage] = useState(0);
  const [importLimitMb, setImportLimitMb] = useState<number>(() => {
    try { const saved = Number(localStorage.getItem("veil.importLimitMb")); return [20, 50, 100].includes(saved) ? saved : 100; }
    catch { return 100; }
  });
  useEffect(() => {
    try { localStorage.setItem("veil.importLimitMb", String(importLimitMb)); } catch { /* Optional preference. */ }
  }, [importLimitMb]);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const [selectionHint, setSelectionHint] = useState(false);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [text, setText] = useState(
    "John Smith lives in New York. Contact him at john.smith@example.com."
  );

  const [detections, setDetections] = useState<ReviewedDetection[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [analyzed, setAnalyzed] = useState(false);
  const [reviewError, setReviewError] = useState("");
  const [documentEdited, setDocumentEdited] = useState(false);
  const [importedDocument, setImportedDocument] = useState<ParsedDocument | null>(null);
  const [importing, setImporting] = useState(false);
  const [importError, setImportError] = useState("");

  const [selectedEntities, setSelectedEntities] = useState<string[]>(() => {
    try {
      const saved = JSON.parse(localStorage.getItem("veil.detectionCategories") ?? "null");
      if (Array.isArray(saved)) return saved.filter((value) => PRIVACY_CATEGORIES.some((category) => category.value === value));
    } catch { /* Use defaults when local preferences are unavailable. */ }
    return PRIVACY_CATEGORIES.slice(0, 5).map((category) => category.value);
  });

  useEffect(() => {
    try { localStorage.setItem("veil.detectionCategories", JSON.stringify(selectedEntities)); }
    catch { /* Detection remains available without saved preferences. */ }
  }, [selectedEntities]);

  const [manualSelection, setManualSelection] =
    useState<ManualSelection | null>(null);
  const [manualCategory, setManualCategory] = useState("PERSON");
  const [manualError, setManualError] = useState("");

  const [validation, setValidation] = useState<{
    snapshot: string;
    success: boolean;
    message: string;
  } | null>(null);

  const reviewSnapshot = JSON.stringify([text, detections, workspaceView]);

  const refreshVault = useCallback(() => {
    const request = ++vaultLoadRef.current;
    return invoke<VaultStatus>("vault_status").then(async (status) => {
      if (request !== vaultLoadRef.current) return;
      if (status.unlocked) {
        const [records, identities, rules] = await Promise.all([
          invoke<ProcessedRecord[]>("list_vault_records"),
          invoke<SavedIdentity[]>("list_saved_identities"),
          invoke<DetectionRule[]>("list_detection_rules"),
        ]);
        if (request !== vaultLoadRef.current) return;
        setVaultRecords(records);
        setSavedIdentities(identities);
        setSavedRules(rules);
      } else {
        setVaultRecords([]);
        setSavedIdentities([]);
        setRestoredText(null);
      }
      if (status.exists) setVaultProtection(status.code_required ? "passphrase" : status.authentication_required ? "system" : "account");
      else if (!status.account_supported) setVaultProtection("passphrase");
      setVaultStatus(status);
    }).catch((error) => {
      if (request === vaultLoadRef.current) setVaultError(String(error));
    });
  }, []);

  useEffect(() => {
    let active = true;
    invoke<VaultStatus>("vault_status").then((status) => {
      if (active) { setVaultStatus(status); if (!status.account_supported) setVaultProtection("passphrase"); else if (status.exists) setVaultProtection(status.code_required ? "passphrase" : status.authentication_required ? "system" : "account"); if (status.unlocked) void refreshVault(); }
    }).catch((error) => { if (active) setVaultError(String(error)); });
    return () => { active = false; };
  }, [refreshVault]);

  async function unlockVault(event: React.FormEvent) {
    event.preventDefault();
    const password = vaultPasswordRef.current?.value ?? "";
    const confirmation = vaultConfirmRef.current?.value ?? "";
    const needsCode = vaultStatus.code_required || (!vaultStatus.exists && (vaultProtection === "passphrase" || !vaultStatus.account_supported));
    if (needsCode && !vaultStatus.exists && password !== confirmation) {
      setVaultError("The passphrases do not match.");
      return;
    }
    setVaultBusy(true);
    setVaultError("");
    try {
      await invoke("unlock_vault", { password: needsCode ? password : "", create: !vaultStatus.exists, useAccount: vaultStatus.exists ? !vaultStatus.code_required : vaultProtection !== "passphrase" && vaultStatus.account_supported, requireAuthentication: vaultProtection === "system" });
      await refreshVault();
    } catch (error) { setVaultError(String(error)); }
    finally {
      if (vaultPasswordRef.current) vaultPasswordRef.current.value = "";
      if (vaultConfirmRef.current) vaultConfirmRef.current.value = "";
      setVaultBusy(false);
    }
  }

  async function lockVault() {
    ++vaultLoadRef.current;
    setVaultBusy(true);
    try {
      await invoke("lock_vault");
      setRestoredText(null);
      setSavedIdentities([]);
      setVaultRecords([]);
      setProcessed(null);
      await refreshVault();
    } catch (error) { setVaultError(String(error)); }
    finally { setVaultBusy(false); }
  }

  async function addIdentity(event: React.FormEvent) {
    event.preventDefault();
    setVaultBusy(true);
    setVaultError("");
    try {
      await invoke("save_identity", {
        original: identityNameRef.current?.value ?? "",
        entityType: identityCategory,
      });
      if (identityNameRef.current) identityNameRef.current.value = "";
      await refreshVault();
    } catch (error) { setVaultError(String(error)); }
    finally { setVaultBusy(false); }
  }

  async function applySavedIdentity(id: string) {
    setLoading(true);
    setVaultError("");
    try {
      const found = await invoke<ValidatedDetection[]>("find_saved_identity", { id, text });
      if (!found.length) {
        setVaultError("No matches in the current document. Paste or upload a document containing this identity first.");
        return;
      }
      const additions = found.filter((hit) => !detections.some((item) =>
        item.status !== "rejected" && rangesOverlap(hit, item)));
      setDetections((current) => [...current, ...additions.map((item) => ({
        ...item, status: "pending" as ReviewStatus, confidence: null,
      }))].sort((a, b) => a.start - b.start));
      setAnalyzed(true);
      setProcessed(null);
      setValidation(null);
      setReviewError(""); setReviewNotice("");
      setWorkspaceView("Pseudonymize");
    } catch (error) { setVaultError(String(error)); }
    finally { setLoading(false); }
  }


  async function changeProtection(event: React.FormEvent) {
    event.preventDefault();
    const password = protectionPasswordRef.current?.value ?? "";
    if (vaultProtection === "passphrase" && password !== protectionConfirmRef.current?.value) {
      setVaultError("The passphrases do not match."); return;
    }
    setVaultBusy(true); setVaultError(""); setSettingsMessage("");
    try {
      await invoke("set_vault_protection", { useAccount: vaultProtection !== "passphrase", requireAuthentication: vaultProtection === "system", password });
      await refreshVault();
      setSettingsMessage("Vault protection updated. Saved identities and documents were kept.");
    } catch (error) { setVaultError(String(error)); }
    finally {
      if (protectionPasswordRef.current) protectionPasswordRef.current.value = "";
      if (protectionConfirmRef.current) protectionConfirmRef.current.value = "";
      setVaultBusy(false);
    }
  }

  async function saveRules(event: React.FormEvent) {
    event.preventDefault();
    const rules = (rulesRef.current?.value ?? "").split("\n").map((phrase) => phrase.trim())
      .filter(Boolean).map((phrase) => ({ phrase, entity_type: "INDIRECT_IDENTIFIER" }));
    setVaultBusy(true); setVaultError(""); setSettingsMessage("");
    try {
      await invoke("save_detection_rules", { rules });
      setSavedRules(rules);
      invalidateDetectionReview();
      if (rules.length) setSelectedEntities((current) => [...new Set([...current, "INDIRECT_IDENTIFIER"])]);
      setSettingsMessage("Custom phrases saved.");
    } catch (error) { setVaultError(String(error)); }
    finally { setVaultBusy(false); }
  }

  function clearWorkspace() {
    if (!window.confirm("Clear the current document, review decisions, and preview? Saved Vault data will be kept.")) return;
    replaceDocumentText("");
    setImportedDocument(null); setProcessed(null); setRestoredText(null);
    setExportMessage(""); setReviewQuery(""); setReviewFilter("all");
  }

  async function resetVault(event: React.FormEvent) {
    event.preventDefault();
    const confirmation = resetConfirmationRef.current?.value ?? "";
    setVaultBusy(true); setResetError("");
    try {
      await invoke("reset_vault", { confirmation });
      ++vaultLoadRef.current;
      setVaultRecords([]); setSavedIdentities([]); setSavedRules([]);
      if (rulesRef.current) rulesRef.current.value = "";
      if (identityNameRef.current) identityNameRef.current.value = "";
      setExportMessage("");
      setProcessed(null); setRestoredText(null); setIdentityQuery("");
      setSettingsMessage("Vault reset. Saved identities, mappings, rules, and history were cleared.");
      await refreshVault();
      resetDialogRef.current?.close();
      if (resetConfirmationRef.current) resetConfirmationRef.current.value = "";
    } catch (error) { setResetError(String(error)); }
    finally { setVaultBusy(false); }
  }

  const onNativeDrop = useEffectEvent((payload: { type: string; paths?: string[]; position?: { x: number; y: number } }, scale: number) => {
    const area = dropRegionRef.current?.getBoundingClientRect();
    const point = payload.position;
    const inside = !!area && !!point && point.x / scale >= area.left && point.x / scale <= area.right
      && point.y / scale >= area.top && point.y / scale <= area.bottom;
    if (payload.type === "leave") { setDragging(false); return; }
    if (payload.type === "enter" || payload.type === "over") {
      setDragging(inside && !loading); return;
    }
    if (payload.type === "drop") {
      setDragging(false);
      if (!inside || loading || workspaceView === "Vault" || workspaceView === "Settings") return;
      if (payload.paths?.length !== 1) { setImportError("Drop one document at a time."); return; }
      void uploadDocument(payload.paths[0]);
    }
  });

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    const connect = async () => {
      try {
        const scale = await getCurrentWindow().scaleFactor();
        const stop = await getCurrentWebview().onDragDropEvent((event) => onNativeDrop(event.payload, scale));
        if (disposed) stop(); else unlisten = stop;
      } catch { /* Browser previews use the native upload button in the desktop app. */ }
    };
    void connect();
    return () => { disposed = true; unlisten?.(); };
  }, []);


  async function exportText(value: string, format = "txt") {
    setExportMessage("");
    try {
      const saved = await invoke<boolean>("export_processed_text", { text: value, format });
      setExportMessage(saved ? "Text file saved." : "");
    } catch (error) { setExportMessage(String(error)); }
  }

  async function restoreRecord(id: string) {
    setVaultError("");
    setRestoredText(null);
    try { setRestoredText(await invoke<string>("restore_vault_record", { id })); }
    catch (error) { setVaultError(String(error)); }
  }

  async function clearVault() {
    if (!window.confirm("Clear saved document history? Document restoration will no longer be available. Your saved identities and pseudonyms will be kept.")) return;
    try {
      await invoke("clear_vault_records");
      setVaultRecords([]);
      setRestoredText(null);
      setProcessed(null);
      setVaultError("");
    } catch (error) { setVaultError(String(error)); }
  }



  const filteredDetections = useMemo(() => detections.filter(item =>
    (reviewFilter === "all" || item.status === reviewFilter)
    && (reviewCategory === "all" || item.entity_type === reviewCategory)
    && item.text.toLowerCase().includes(reviewQuery.trim().toLowerCase())
  ), [detections, reviewFilter, reviewCategory, reviewQuery]);
  const reviewGroups = useMemo(() => groupDetections(filteredDetections), [filteredDetections]);
  const visibleDetections = reviewGroups.map(group => group.find(item => item.id === activeOccurrences[matchKey(group[0])]) ?? group[0]);
  const textBytes = useMemo(() => new TextEncoder().encode(text).length, [text]);
  const reviewPageCount = Math.max(1, Math.ceil(reviewGroups.length / 50));
  const visibleReviewPage = Math.min(reviewPage, reviewPageCount - 1);
  const editingDetection = detections.find((item) => item.id === editingId);

  function cancelEdit() {
    setEditingId(null);
    setManualSelection(null);
    setReviewError(""); setReviewNotice("");
  }

  function revealDetection(item: ReviewedDetection) {
    const input = inputRef.current;
    if (!input || item.needsCorrection) return;
    const chars = Array.from(text);
    input.focus();
    input.setSelectionRange(chars.slice(0, item.start).join("").length, chars.slice(0, item.end).join("").length);
    // Focus the selection within a long textarea as well as the page.
    input.blur();
    input.focus();
    input.scrollIntoView({ behavior: "smooth", block: "center" });
  }

  function nextOccurrence(item: ReviewedDetection) {
    const group = reviewGroups.find(group => matchKey(group[0]) === matchKey(item));
    if (!group) return;
    const next = group[(group.findIndex(hit => hit.id === item.id) + 1) % group.length];
    setActiveOccurrences(current => ({ ...current, [matchKey(item)]: next.id }));
    revealDetection(next);
  }

  function startEdit(item: ReviewedDetection) {
    setEditingId(item.id);
    setManualSelection(null);
    setSelectionHint(false);
    setReviewError(""); setReviewNotice("");
    setManualError("");
    const input = inputRef.current;
    if (input) {
      input.focus();
      // Convert code-point offsets back to the textarea's UTF-16 offsets.
      const chars = Array.from(text);
      input.setSelectionRange(chars.slice(0, item.start).join("").length, chars.slice(0, item.end).join("").length);
      input.scrollIntoView({ behavior: "smooth", block: "center" });
    }
  }

  function invalidateDetectionReview() {
    setDetections([]); setAnalyzed(false); setProcessed(null); setValidation(null);
    setSuppressed([]); setEditingId(null); setManualSelection(null);
    setError(""); setReviewError(""); setReviewNotice(""); setDocumentEdited(false);
    setReviewPage(0);
  }

  function toggleEntity(entity: string) {
    if (loading) {
      return;
    }

    invalidateDetectionReview();
    setSelectedEntities((current) =>
      current.includes(entity)
        ? current.filter((item) => item !== entity)
        : [...current, entity]
    );
  }

  function updateText(value: string) {
    if (value === text) return;
    if (importedDocument) {
      setDetections([]);
      setAnalyzed(false);
      setDocumentEdited(false);
    } else {
      setDetections((current) => rebaseDetections(text, value, current));
      if (analyzed) setDocumentEdited(true);
    }
    setText(value);
    setSuppressed([]);
    setEditingId(null);
    setSelectionHint(false);
    setError("");
    setValidation(null);
    setReviewError(""); setReviewNotice("");
    setManualError("");
    setManualSelection(null);
  }

  function replaceDocumentText(value: string) {
    setText(value);
    setSuppressed([]);
    setDetections([]);
    setAnalyzed(false);
    setDocumentEdited(false);
    setEditingId(null);
    setSelectionHint(false);
    setManualSelection(null);
    setValidation(null);
    setError("");
    setReviewError(""); setReviewNotice("");
    setManualError("");
    setImportError("");
  }

  async function uploadDocument(droppedPath?: string) {
    if (loading || importInFlightRef.current) return;
    importInFlightRef.current = true;
    setLoading(true);
    setImporting(true);
    setImportError("");
    try {
      const document = droppedPath ? await invoke<ParsedDocument>("parse_dropped_document", { path: droppedPath, maxFileMb: importLimitMb }) : await invoke<ParsedDocument | null>("parse_document", { maxFileMb: importLimitMb });
      // Cancellation and failures preserve the current text and review decisions.
      if (document) {
        replaceDocumentText(document.text);
        setImportedDocument(document);
      }
    } catch (error) {
      setImportError(importErrorMessage(error));
    } finally {
      setLoading(false);
      setImporting(false);
      importInFlightRef.current = false;
    }
  }

  function removeDocument() {
    if (loading) return;
    setImportedDocument(null);
    replaceDocumentText("");
    inputRef.current?.focus();
  }

  function addManualDetection() {
    if (!manualSelection || !analyzed || loading) {
      return;
    }

    const overlaps = detections.some(
      (item) =>
        item.status !== "rejected" &&
        rangesOverlap(manualSelection, item)
    );

    if (overlaps) {
      setManualError(
        "This selection overlaps an existing detection. Reject that detection first if you want to replace it."
      );
      return;
    }

    const newDetection: ReviewedDetection = {
      id: crypto.randomUUID(),
      text: manualSelection.text,
      start: manualSelection.start,
      end: manualSelection.end,
      entity_type: manualCategory,
      confidence: null,
      status: "pending",
    };

    setDetections((current) =>
      [...current, newDetection].sort(
        (first, second) => first.start - second.start
      )
    );

    setManualSelection(null);
    setManualError("");
    setReviewError(""); setReviewNotice("");
  }

  function updateDetectionBoundary(id: string) {
    if (!manualSelection || !analyzed || loading) {
      return;
    }

    const target = detections.find((item) => item.id === id);

    if (!target) {
      return;
    }

    const overlaps = detections.some(
      (item) =>
        item.id !== id &&
        item.status !== "rejected" &&
        rangesOverlap(manualSelection, item)
    );

    if (overlaps) {
      setReviewError(
        "The selected text overlaps another active detection. Reject that detection first."
      );
      return;
    }

    setDetections((current) =>
      current
        .map((item) =>
          item.id === id
            ? {
                ...item,
                text: manualSelection.text,
                start: manualSelection.start,
                end: manualSelection.end,
                confidence: null,
                needsCorrection: false,
                status: "pending" as ReviewStatus,
              }
            : item
        )
        .sort((first, second) => first.start - second.start)
    );

    setEditingId(null);
    setManualSelection(null);
    setManualError("");
    setReviewError(""); setReviewNotice("");
  }

  function updateDetectionCategory(id: string, entityType: string) {
    if (loading) {
      return;
    }

    setReviewError(""); setReviewNotice("");
    setManualError("");

    setDetections((current) =>
      current.map((item) =>
        item.id === id
          ? {
              ...item,
              entity_type: entityType,
              status:
                item.status === "rejected" ? "rejected" : "pending",
            }
          : item
      )
    );
  }

  function applyDecision(ids: string[], status: ReviewStatus, repeated: boolean) {
    if (loading || editingId !== null) return;
    const result = applyReviewDecision(text, detections, ids, status, repeated, repeated && status === "accepted");
    setDetections(result.items);
    setReviewError(""); setManualError("");
    setReviewNotice(
      String(result.changed) + " item(s) " + (status === "pending" ? "reset for review" : status) + "."
      + (result.added ? " Found and included " + result.added + " additional exact match(es)." : "")
      + (result.skipped ? " " + result.skipped + " item(s) need correction or overlap another detection." : "")
    );
  }

  function updateReviewStatus(id: string, status: ReviewStatus) {
    applyDecision([id], status, status !== "pending" && linkRepeated);
  }

  async function detectPII() {
    if (loading || !text.trim() || textBytes > 500_000 || selectedEntities.length === 0) {
      return;
    }

    setLoading(true);
    setError("");
    setValidation(null);
    setReviewError(""); setReviewNotice("");
    setManualError("");
    setManualSelection(null);
    setDetections([]);
    setSuppressed([]);
    setAnalyzed(false);
    setReviewPage(0);
    setEditingId(null);
    setSelectionHint(false);

    try {
      const response = await invoke<DetectionResponse>("detect_with_rules", {
        text,
        entities: selectedEntities,
        options: {
          language: documentLanguage,
        },
      });

      setDetections(
        response.detections.map((item, index) => ({
          ...item,
          id: `detection-${index}`,
          status: "pending",
        }))
      );

      setSuppressed(response.suppressed ?? []);
      setAnalyzed(true);
      setDocumentEdited(false);
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }


  async function validateAcceptedDetections(includePending = false) {
    if (loading || !analyzed || editingId !== null) return;
    if (workspaceView === "Pseudonymize" && !vaultStatus.unlocked) {
      vaultPromptRef.current?.showModal();
      return;
    }
    let accepted: ValidatedDetection[];
    let nextDetections = detections;
    try {
      const chosen = prepareProcessing(text, detections, includePending);
      accepted = chosen.map(({ id, entity_type, text, start, end }) => ({ id, entity_type, text, start, end }));
      if (includePending) {
        const ids = new Set(chosen.map(item => item.id));
        nextDetections = detections.map(item => ids.has(item.id) ? { ...item, status: "accepted" as ReviewStatus } : item);
        setDetections(nextDetections);
      }
    } catch (error) {
      setValidation({snapshot: reviewSnapshot, success: false, message: error instanceof Error ? error.message : String(error)});
      return;
    }
    const snapshot = JSON.stringify([text, nextDetections, workspaceView]);
    setProcessed(null);
    setLoading(true);
    setValidation(null);

    try {
      const mode = workspaceView === "Pseudonymize" ? "pseudonymize" : "anonymize";
      const result = await invoke<ProcessedRecord>("process_reviewed_text", {
        text, detections: accepted, mode,
      });
      setProcessed({record: result, snapshot, mode: workspaceView});
      setExportMessage("");

      setValidation({
        snapshot,
        success: true,
        message: `${result.count} accepted item(s) processed.`,
      });
    } catch (err) {
      setValidation({
        snapshot,
        success: false,
        message: `Processing failed: ${String(err)}`,
      });
    } finally {
      setLoading(false);
    }
  }

  return (
    <div className="app" aria-busy={loading}>
      <header className="sidebar">
        <div className="brand">
          <div className={`vault-mark vault-reactive ${workspaceView === "Vault" ? "is-open" : ""}`} aria-hidden="true">
            <div className="vault-door"><span className="vault-wheel"><i /><i /></span><span className="vault-handle" /></div>
          </div>
          <span>VEIL</span>
        </div>
        <nav aria-label="Main navigation">
          {["Anonymize", "Pseudonymize", "Vault", "Settings"].map((view) => (
            <button key={view} disabled={loading || vaultBusy} className={`nav-item ${workspaceView === view ? "active" : ""}`} aria-current={workspaceView === view ? "page" : undefined} onClick={() => { setWorkspaceView(view); void refreshVault(); setRestoredText(null); setVaultError(""); setExportMessage(""); }}>
              <Icon name={view === "Vault" ? "vault" : view === "Settings" ? "settings" : view === "Pseudonymize" ? "database" : "home"} />{view}
            </button>
          ))}
        </nav>
        <div className="sidebar-bottom"><span className="local-dot" />Processed locally on your device</div>
        <div className="sidebar-wave" aria-hidden="true" />
      </header>

      <main className="workspace">
        <header className="intro">
          <h1>{workspaceView === "Anonymize" ? "Anonymization workspace" : workspaceView === "Pseudonymize" ? "Pseudonymization workspace" : workspaceView}</h1>
          <p>{workspaceView === "Vault" ? "Your saved people, pseudonyms, and processed documents." : workspaceView === "Settings" ? "Manage your detection preferences." : "Inspect your documents and review sensitive information before sharing."}</p>
        </header>

        {(workspaceView === "Vault" || workspaceView === "Settings") && <section className="view-details">
          {workspaceView === "Vault" && <>
            <div className="vault-display" aria-hidden="true"><div className="vault-mark is-open"><div className="vault-door"><span className="vault-wheel"><i /><i /></span><span className="vault-handle" /></div></div></div>
            <h2>Your Vault</h2>
            <p>People and identity mappings are saved locally in an encrypted Vault. Their pseudonyms remain the same when you reopen Veil.</p>
            {vaultError && <div className="error" role="alert">{vaultError}</div>}
            {!vaultStatus.unlocked ? <form className="vault-unlock" onSubmit={unlockVault}>
              <h3>{vaultStatus.exists ? "Unlock your Vault" : "Create your Vault"}</h3>
              {(vaultStatus.code_required || (!vaultStatus.exists && (vaultProtection === "passphrase" || !vaultStatus.account_supported))) && <>
                <label>Passphrase<input ref={vaultPasswordRef} type="password" autoComplete={vaultStatus.exists ? "current-password" : "new-password"} required minLength={vaultStatus.exists ? 1 : 12} maxLength={1024} disabled={vaultBusy} /></label>
                {!vaultStatus.exists && <label>Confirm passphrase<input ref={vaultConfirmRef} type="password" autoComplete="new-password" required minLength={12} maxLength={1024} disabled={vaultBusy} /></label>}
              </>}
              <p>{vaultStatus.code_required ? "Enter your Vault passphrase. You can change password protection in Settings after unlocking." : vaultProtection === "passphrase" ? "Keep this passphrase to reopen your encrypted Vault." : vaultProtection === "system" ? "Your computer will ask you to verify with Windows Hello or macOS sign-in. VEIL never receives your system password or biometric data." : "Protected by your signed-in account on this computer, with no separate Vault password."}</p>
              <button className="primary" type="submit" disabled={vaultBusy}>{vaultBusy ? "Opening Vault..." : vaultStatus.authentication_required || (!vaultStatus.exists && vaultProtection === "system") ? "Verify with system sign-in" : vaultStatus.exists ? "Unlock Vault" : "Create Vault"}</button>
            </form> : <>
              <div className="vault-heading"><h3>Saved identities</h3><button className="secondary-button" disabled={vaultBusy || loading} onClick={lockVault}><Icon name="vault" />Lock Vault</button></div>
              <form className="identity-form" onSubmit={addIdentity}>
                <label>Name or identifying text<input ref={identityNameRef} placeholder="John Smith" required maxLength={1000} disabled={vaultBusy || loading} /></label>
                <label>Category<select value={identityCategory} onChange={(event) => setIdentityCategory(event.target.value)} disabled={vaultBusy || loading}>{PRIVACY_CATEGORIES.map((category) => <option key={category.value} value={category.value}>{category.label}</option>)}</select></label>
                <button className="primary" type="submit" disabled={vaultBusy || loading}><Icon name="plus" />Save identity</button>
              </form>
              <input className="identity-search" type="search" aria-label="Search saved identities" placeholder="Search by name or pseudonym" value={identityQuery} onChange={(event) => setIdentityQuery(event.target.value)} />
              <div className="identity-list">{savedIdentities.filter((identity) => (identity.original + " " + identity.pseudonym).toLowerCase().includes(identityQuery.toLowerCase())).map((identity) =>
                <div className="identity-row" key={identity.id}><div><strong>{identity.original}</strong><span>{PRIVACY_CATEGORIES.find((category) => category.value === identity.entity_type)?.label}</span></div><code>{identity.pseudonym}</code><button className="secondary-button" disabled={loading || vaultBusy || !text.trim()} onClick={() => applySavedIdentity(identity.id)}>Find in current document</button></div>
              )}</div>
              {!savedIdentities.length && <p>Save an identity above, or pseudonymize reviewed information to add it automatically.</p>}
              {savedIdentities.length > 0 && !savedIdentities.some((identity) => (identity.original + " " + identity.pseudonym).toLowerCase().includes(identityQuery.toLowerCase())) && <p>No saved identities match your search.</p>}
              <div className="vault-heading"><h3>Document history</h3><button className="secondary-button" disabled={vaultBusy || loading || !vaultRecords.length} onClick={clearVault}>Clear document history</button></div>
              <p>Clearing document history keeps your saved people and pseudonyms.</p>
              {vaultRecords.length === 0 ? <p>No processed documents saved yet.</p> : vaultRecords.map((record, index) => <div className="vault-record" key={record.id}>
                <strong>Document {vaultRecords.length - index}</strong>
                <span>{record.mode === "pseudonymize" ? "Pseudonymized" : "Anonymized"} · {record.count} replacements</span>
                <button className="secondary-button" disabled={vaultBusy} onClick={() => exportText(record.text)}><Icon name="document" />Export TXT</button>
                <button className="secondary-button" disabled={vaultBusy} onClick={() => exportText(record.text, "pdf")}>Export PDF</button>
                {record.mode === "pseudonymize" && <button className="secondary-button" disabled={vaultBusy} onClick={() => restoreRecord(record.id)}>Restore original</button>}
              </div>)}
              {restoredText !== null && <div className="processed-output"><h3>Restored original</h3><textarea readOnly dir="auto" aria-label="Restored original text" value={restoredText} /><button className="secondary-button" onClick={() => setRestoredText(null)}>Hide original</button></div>}
              {exportMessage && <p role="status">{exportMessage}</p>}
            </>}
          </>}

          {workspaceView === "Settings" && <>
            <h2>Detection categories</h2>
            <p>Select the types of information to find and protect. Extra categories are optional.</p>
            <div className="settings-category-grid">{PRIVACY_CATEGORIES.map((category) => <label className="privacy-category" key={category.value}><input type="checkbox" disabled={loading} checked={selectedEntities.includes(category.value)} onChange={() => toggleEntity(category.value)} /><span>{category.label}</span></label>)}</div>
            <p className="selection-hint">Indirect identifiers include distinctive descriptions such as a rare role combined with a place or nationality. Suggestions need your review; uniqueness cannot be determined reliably from a document alone.</p>
            <h3>Custom identifying phrases</h3>
            <p>Enter one phrase per line to suggest it for review. Phrases are matched literally and saved in your encrypted Vault.</p>
            {vaultStatus.unlocked ? <form className="settings-form" onSubmit={saveRules}>
              <label>Identifying phrases<textarea ref={rulesRef} aria-label="Custom identifying phrases" defaultValue={savedRules.map((rule) => rule.phrase).join("\n")} placeholder="A distinctive job title or description" /></label>
              <button className="secondary-button" disabled={vaultBusy || loading}>Save phrases</button>
            </form> : <button className="secondary-button" onClick={() => setWorkspaceView("Vault")}>Open Vault to manage phrases</button>}
            <h3>Document import</h3>
            <div className="settings-form">
              <label>Maximum file size <select aria-label="Maximum document file size" value={importLimitMb} disabled={loading || importing} onChange={event => setImportLimitMb(Number(event.target.value))}>
                <option value={20}>20 MB</option><option value={50}>50 MB</option><option value={100}>100 MB</option>
              </select></label>
              <p>Local files only. Extracted text can contain up to 500,000 UTF-8 bytes. Large text is detected in overlapping chunks; PDF page and archive-expansion limits still apply. Oversized text is rejected without truncation.</p>
            </div>
            <h3>Vault sign-in</h3>
            <form className="settings-form" onSubmit={changeProtection}>
              <label><input type="radio" name="vault-protection" value="system" checked={vaultProtection === "system"} disabled={!vaultStatus.account_supported || vaultBusy || (vaultStatus.exists && !vaultStatus.unlocked)} onChange={() => setVaultProtection("system")} />Windows Hello / macOS sign-in</label>
              <p>Verify with Windows Hello (PIN, fingerprint or face), or Touch ID / your Mac login password. Verification stays on your computer and is required each time you unlock the Vault.</p>
              <label><input type="radio" name="vault-protection" value="account" checked={vaultProtection === "account"} disabled={!vaultStatus.account_supported || vaultBusy || (vaultStatus.exists && !vaultStatus.unlocked)} onChange={() => setVaultProtection("account")} />No extra Vault password</label>
              <p>Opens automatically through your signed-in Windows or macOS account. Saved data remains encrypted on this computer.</p>
              <label><input type="radio" name="vault-protection" value="passphrase" checked={vaultProtection === "passphrase"} disabled={vaultBusy || (vaultStatus.exists && !vaultStatus.unlocked)} onChange={() => setVaultProtection("passphrase")} />Separate Vault passphrase</label>
              {vaultProtection === "passphrase" && vaultStatus.unlocked && <>
                <label>New passphrase<input type="password" ref={protectionPasswordRef} autoComplete="new-password" required minLength={12} maxLength={1024} /></label>
                <label>Confirm passphrase<input type="password" ref={protectionConfirmRef} autoComplete="new-password" required minLength={12} maxLength={1024} /></label>
              </>}
              {vaultStatus.exists && !vaultStatus.unlocked && <p>Unlock the Vault before changing its password setting.</p>}
              {!vaultStatus.exists && <p>Your choice will be used when you create the Vault.</p>}
              {vaultStatus.unlocked && <button className="primary" disabled={vaultBusy || loading}>Apply Vault setting</button>}
            </form>
            <h3>Clear data</h3>
            <div className="data-actions">
              <button className="secondary-button" disabled={loading} onClick={clearWorkspace}>Clear workspace</button>
              <button className="secondary-button" disabled={!vaultStatus.unlocked || vaultBusy || loading} onClick={clearVault}>Clear document history</button>
              <button className="danger-button" disabled={vaultBusy || loading} onClick={() => resetDialogRef.current?.showModal()}>Reset entire Vault</button>
            </div>
            {vaultError && <div className="error" role="alert">{vaultError}</div>}
            {settingsMessage && <p role="status">{settingsMessage}</p>}
          </>}

        </section>}
        <div hidden={workspaceView === "Vault" || workspaceView === "Settings"}>
        <div className={`mode-indicator ${workspaceView === "Pseudonymize" ? "pseudonymize-mode" : ""}`} role="status" aria-live="polite">
          <Icon name={workspaceView === "Pseudonymize" ? "database" : "home"} />
          <div><strong>{workspaceView === "Pseudonymize" ? "Pseudonymization" : "Anonymization"}</strong>
          <p>{workspaceView === "Pseudonymize" ? "Reversible replacement using pseudonyms and a secure identity mapping." : "Irreversible removal or replacement of identifying information."}</p>
          <span>Accept or reject detections, then process the accepted information.</span></div>
        </div>
        {workspaceView === "Pseudonymize" && !vaultStatus.unlocked && <div className="vault-lock-note"><Icon name="vault" /><span>Unlock your Vault to pseudonymize documents and reuse saved identities.</span><button className="secondary-button" onClick={() => setWorkspaceView("Vault")}>Open Vault</button></div>}
        <section ref={dropRegionRef} aria-label="Document and detection settings" className={`input-section ${dragging ? "drag-active" : ""}`} id="document">
          <div className="drop-instruction"><Icon name="upload" /><span>{dragging ? "Drop your document here" : "Drop a document here or choose Upload document"}</span></div>
          <div className="input-methods" aria-label="Document input method">
            <button className={`input-method ${!importedDocument ? "selected" : ""}`} aria-pressed={!importedDocument} disabled={loading} onClick={() => {
              setImportedDocument(null);
              setImportError("");
              inputRef.current?.focus();
            }}><Icon name="document" />Paste text</button>
            <button className={`input-method ${importedDocument ? "selected" : ""}`} aria-pressed={!!importedDocument} disabled={loading} onClick={() => void uploadDocument()}><Icon name="upload" />{importing ? "Opening / parsing locally…" : "Upload document"}</button>
            <span className="upload-help">TXT, DOCX, PDF, CSV, XLSX - up to {importLimitMb} MB - local only</span>
          </div>
          {importedDocument && (
            <div className="file-summary">
              <div><strong>{importedDocument.file_name}</strong><span>{importedDocument.file_type.toUpperCase()}{importedDocument.pages !== null ? ` · ${importedDocument.pages} page(s)` : ""}</span></div>
              <button className="secondary-button" disabled={loading} onClick={() => void uploadDocument()}><Icon name="upload" />Replace file</button>
              <button className="secondary-button" disabled={loading} onClick={removeDocument}><Icon name="trash" />Remove file</button>
            </div>
          )}
          {importing && <p className="selection-hint" role="status">Choose a document in the local file picker. Its text will be extracted on this device.</p>}
          {importError && <div className="error" role="alert">{importError}</div>}
          {importedDocument?.warnings.map((warning, index) => <div className="document-warning" role="status" key={index}>{warning}</div>)}
          <div className="document-input" aria-busy={importing}>
            <span className="input-size" id="input-size">{textBytes.toLocaleString()} / 500,000 text bytes</span>
          <textarea
            ref={inputRef}
            aria-label="Document input"
            dir="auto"
            value={text}
            onChange={(event) => updateText(event.target.value)}
            disabled={loading}
            aria-describedby={editingDetection ? "input-size edit-instructions" : "input-size"}
            maxLength={500000}
            placeholder="Paste your text here…"
            onSelect={(event) => {
              const input = event.currentTarget;
              const start = input.selectionStart;
              const end = input.selectionEnd;
              const selectedText = input.value.slice(start, end);
              setManualError("");
              if (!analyzed || loading || !selectedText.trim()) {
                setManualSelection(null);
                return;
              }
              setSelectionHint(false);
              setManualSelection({
                start: Array.from(input.value.slice(0, start)).length,
                end: Array.from(input.value.slice(0, end)).length,
                text: selectedText,
              });
            }}
          />
          </div>
          {editingDetection && (
            <div className="edit-panel" aria-label="Correct detection">
              <div id="edit-instructions" role="status">
                <strong>Editing “{editingDetection.text}”</strong>
                <p>Select the corrected text in the document above, then save your correction.</p>
                <p>Replacement: <strong>{manualSelection?.text ?? "Select text above"}</strong></p>
              </div>
              <div className="edit-actions">
                <button className="secondary-button" onClick={cancelEdit} disabled={loading}>Cancel</button>
                <button className="primary" disabled={loading || !manualSelection} onClick={() => updateDetectionBoundary(editingDetection.id)}>Save correction</button>
              </div>
              {reviewError && <div className="error" role="alert">{reviewError}</div>}
            </div>
          )}
          <p className="detection-help">Models run locally. Phone detection checks all supported regions automatically. Scores are recognizer signals, not probabilities. Automatic indirect-description suggestions currently cover English; use custom phrases for other languages.</p>
          <div className="detect-toolbar">
            <label className="document-language">Document language<select aria-label="Document language" disabled={loading} value={documentLanguage} onChange={event => { setDocumentLanguage(event.target.value); invalidateDetectionReview(); }}>
              <option value="en">English</option><option value="fr">Français</option><option value="es">Español</option><option value="ar">العربية</option>
            </select></label>
            <details className="category-picker"><summary>Data categories ({selectedEntities.length})</summary>
            <div className="privacy-categories">
              {PRIVACY_CATEGORIES.map((category) => (
                <label key={category.value} className="privacy-category">
                  <input type="checkbox" disabled={loading} checked={selectedEntities.includes(category.value)} onChange={() => toggleEntity(category.value)} />
                  <span>{category.label}</span>
                </label>
              ))}
            </div></details>
            <button className="primary detect-button" onClick={detectPII} disabled={loading || !text.trim() || textBytes > 500_000 || selectedEntities.length === 0}>
              {!importing && loading && !analyzed ? "Detecting…" : "Detect"}<Icon name="arrow" />
            </button>
          </div>
          {textBytes > 500_000 && <div className="error" role="alert">Text exceeds 500,000 UTF-8 bytes. Split the document before detection.</div>}
          {selectedEntities.length === 0 && <div className="error" role="alert">Select at least one sensitive data category.</div>}
          {error && <div className="error" role="alert">Detection failed: {error}</div>}
        </section>

        <section className="results-panel" id="review" aria-labelledby="results-heading">
          <div className="panel-header">
            <div><h2 id="results-heading">Detected Information</h2><p>Review and refine the detected items before continuing.</p></div>
            <span className="result-count">{detections.filter((item) => item.status !== "pending").length} / {detections.length} reviewed</span>
          </div>
          {documentEdited && <p className="selection-hint" role="status">Your review has been preserved. Items affected by text changes need review again. Newly added information is not automatically detected; select it to add it manually.</p>}
          {reviewError && !editingDetection && <div className="error" role="alert">{reviewError}</div>}
          <div className="review-toolbar">
            <input type="search" aria-label="Search detected information" placeholder="Search detected information" value={reviewQuery} onChange={(event) => { setReviewQuery(event.target.value); setReviewPage(0); }} />
            <select aria-label="Filter review status" value={reviewFilter} onChange={(event) => { setReviewFilter(event.target.value); setReviewPage(0); }}>
              <option value="all">All detections</option>
              <option value="pending">Pending review</option>
              <option value="accepted">Accepted</option>
              <option value="rejected">Rejected</option>
            </select>
            <select aria-label="Filter detection category" value={reviewCategory} onChange={event => { setReviewCategory(event.target.value); setReviewPage(0); }}>
              <option value="all">All categories</option>{PRIVACY_CATEGORIES.map(category => <option key={category.value} value={category.value}>{category.label}</option>)}
            </select>
            <button className="secondary-button" disabled={!reviewQuery && reviewFilter === "all" && reviewCategory === "all"} onClick={() => { setReviewQuery(""); setReviewFilter("all"); setReviewCategory("all"); setReviewPage(0); }}>Clear filters</button>
          </div>
          {detections.length > 0 && <div className="bulk-review">
            <div className="bulk-review-actions">
              <button className="secondary-button" disabled={loading || editingId !== null || !filteredDetections.length} onClick={() => applyDecision(filteredDetections.map(item => item.id), "accepted", false)}>Accept all ({filteredDetections.length})</button>
              <button className="secondary-button" disabled={loading || editingId !== null || !filteredDetections.length} onClick={() => applyDecision(filteredDetections.map(item => item.id), "rejected", false)}>Reject all ({filteredDetections.length})</button>
              <span>Applies to all results matching your filters, across every page.</span>
            </div>
            <label className="repeat-choice"><input type="checkbox" checked={linkRepeated} disabled={loading || editingId !== null} onChange={event => setLinkRepeated(event.target.checked)} />Apply individual accept/reject decisions to repeated matches</label>
            <p>Matches use the same text and category. Accepting a match also finds other exact occurrences; previously rejected occurrences and overlapping detections stay unchanged. Turn this off to decide one occurrence at a time.</p>
          </div>}
          {suppressed.length > 0 && <details className="suppressed-results"><summary>{suppressed.length} suggestions excluded during detection</summary><p>Excluded suggestions will stay unchanged. Select text in the document to add an item manually if needed.</p><ul>{suppressed.map((item, index) => <li key={index}><bdi>{item.text}</bdi> · {PRIVACY_CATEGORIES.find(category => category.value === item.entity_type)?.label} · {item.reason}</li>)}</ul></details>}
          {reviewNotice && <p className="review-notice" role="status">{reviewNotice}</p>}
          {detections.length > 0 && filteredDetections.length === 0 && <p role="status" className="selection-hint">No detections match these filters.</p>}
          {detections.length === 0 ? (
            <div className="empty-results">{analyzed ? "No sensitive information detected. Select text above to add missed information." : "Your detected information will appear here. Add your text and select Detect to get started."}</div>
          ) : (
            <div className="detection-list">
              {visibleDetections.slice(visibleReviewPage * 50, (visibleReviewPage + 1) * 50).map((item) => (
                <div className={`detection-item ${item.status} ${editingId === item.id ? "editing" : ""}`} key={item.id}>

                  <div className="entity-text"><button className="detected-text-link" disabled={loading || !!item.needsCorrection} onClick={() => revealDetection(item)} title="Show in document"><strong><bdi>{item.text}</bdi></strong></button>{(() => {
                    const group = reviewGroups.find(group => matchKey(group[0]) === matchKey(item))!;
                    return group.length > 1 && <button className="repeat-count occurrence-button" disabled={loading || editingId !== null} onClick={() => nextOccurrence(item)} title="Show next occurrence in document">{group.length} matches · {group.findIndex(hit => hit.id === item.id) + 1}/{group.length} ↗</button>;
                  })()}<span className="review-status">{item.needsCorrection && item.status !== "rejected" ? "Text removed — edit or reject" : item.status === "pending" ? "Pending review" : item.status === "rejected" ? "Rejected" : "Accepted"}</span></div>
                  <select aria-label={`Category for ${item.text}`} value={item.entity_type} disabled={loading || editingId !== null} onChange={(event) => updateDetectionCategory(item.id, event.target.value)}>
                    {PRIVACY_CATEGORIES.map((category) => <option key={category.value} value={category.value}>{category.label}</option>)}
                  </select>
                  <span className="confidence">{item.confidence === null ? "Manual" : `${(item.confidence * 100).toFixed(0)}%`}</span>
                  <div className="row-actions">
                    <button className="secondary-button edit-button" aria-label={`Edit ${item.text}`} disabled={loading || editingId !== null} onClick={() => startEdit(item)}><Icon name="edit" />Edit</button>
                    <button className={`review-button accept-button ${item.status === "accepted" ? "selected" : ""}`} aria-pressed={item.status === "accepted"} aria-label={`Accept ${item.text}`} disabled={loading || editingId !== null} onClick={() => updateReviewStatus(item.id, "accepted")}>Accept</button>
                    <button className={`review-button reject-button ${item.status === "rejected" ? "selected" : ""}`} aria-pressed={item.status === "rejected"} aria-label={`Reject ${item.text}`} disabled={loading || editingId !== null} onClick={() => updateReviewStatus(item.id, "rejected")}>Reject</button>
                    {item.status !== "pending" && <button className="reset-button" disabled={loading || editingId !== null} onClick={() => updateReviewStatus(item.id, "pending")} aria-label={`Reset review for ${item.text}`}>Reset</button>}
                  </div>
                </div>
              ))}
            </div>
          )}
          {reviewGroups.length > 50 && <div className="review-pagination" aria-label="Detection pages">
            <button className="secondary-button" disabled={loading || visibleReviewPage === 0} onClick={() => setReviewPage(visibleReviewPage - 1)}>Previous</button>
            <span>Page {visibleReviewPage + 1} of {reviewPageCount} - {reviewGroups.length} matching groups</span>
            <button className="secondary-button" disabled={loading || visibleReviewPage + 1 >= reviewPageCount} onClick={() => setReviewPage(visibleReviewPage + 1)}>Next</button>
          </div>}
          <button className="add-button" disabled={!analyzed || loading || editingId !== null} onClick={() => { setSelectionHint(true); inputRef.current?.focus(); }}><Icon name="plus" />Add missed information</button>
          {selectionHint && <p className="selection-hint" role="status">Select the missed information in the document above, choose a category, then select Add information.</p>}
          {analyzed && manualSelection && !editingId && (
            <div className="manual-detection">
              <p>Selected text: <strong>{manualSelection.text}</strong></p>
              <label>Category <select value={manualCategory} disabled={loading} onChange={(event) => { setManualCategory(event.target.value); setManualError(""); }}>{PRIVACY_CATEGORIES.map((category) => <option key={category.value} value={category.value}>{category.label}</option>)}</select></label>
              <button className="primary" onClick={addManualDetection} disabled={loading}>Add information<Icon name="plus" /></button>
              {manualError && <div className="error" role="alert">{manualError}</div>}
            </div>
          )}
          <div className="continue-actions">
            {analyzed && detections.some(item => item.status === "pending") && <p>You can process all pending detections now, or process only the items you accepted. Rejected items stay unchanged. Processing uses the whole document; filters only scope accept/reject.</p>}
            <div className="processing-options">
              <button className="secondary-button" title="Process only accepted detections; pending and rejected items remain unchanged" onClick={() => void validateAcceptedDetections(false)} disabled={loading || editingId !== null || !detections.some(item => item.status === "accepted")}>
                {workspaceView === "Pseudonymize" ? "Pseudonymize accepted" : "Anonymize accepted"}<Icon name="arrow" />
              </button>
              <button className="primary continue-button" title="Process accepted and pending detections; explicitly rejected items remain unchanged" onClick={() => void validateAcceptedDetections(true)} disabled={loading || !analyzed || editingId !== null || !detections.some(item => item.status !== "rejected")}>
                {!importing && loading && analyzed ? "Processing..." : workspaceView === "Pseudonymize" ? "Pseudonymize all detections" : "Anonymize all detections"}<Icon name="arrow" />
              </button>
            </div>
          </div>
          {validation?.snapshot === reviewSnapshot && <div className={validation.success ? "validation-success" : "error"} role={validation.success ? "status" : "alert"}>{validation.message}</div>}
        </section>
          {processed?.snapshot === reviewSnapshot && processed.mode === workspaceView && <section className="processed-output">
            <h2>{processed.record.mode === "pseudonymize" ? "Pseudonymized text" : "Anonymized text"}</h2>
            <p>Review the output before sharing. Rejected and undetected information remains in the text.</p>
            {processed.record.mode === "anonymize" && <p>Anonymized text stays in this workspace and is not saved to the Vault. Export it before closing the app.</p>}
            <textarea readOnly dir="auto" aria-label="Processed text" value={processed.record.text} />
            <button className="primary" onClick={() => exportText(processed.record.text)}><Icon name="document" />Export TXT</button>
            <button className="secondary-button" onClick={() => exportText(processed.record.text, "pdf")}><Icon name="document" />Export PDF</button>
            <p>PDF export creates a new text-layout PDF from this processed preview.</p>
            {exportMessage && <p role="status">{exportMessage}</p>}
          </section>}
        </div>
      </main>
      <dialog ref={vaultPromptRef} className="vault-prompt" aria-labelledby="vault-prompt-title" aria-describedby="vault-prompt-description">
        <div className="vault-prompt-icon"><Icon name="vault" /></div>
        <h2 id="vault-prompt-title">{vaultStatus.exists ? "Unlock your Vault" : "Create your Vault"}</h2>
        <p id="vault-prompt-description">Pseudonymization needs your Vault to securely save identities and restore the original text later. Your current review will be kept.</p>
        <div className="vault-prompt-actions">
          <button className="secondary-button" autoFocus onClick={() => vaultPromptRef.current?.close()}>Not now</button>
          <button className="primary" onClick={() => {
            vaultPromptRef.current?.close();
            setWorkspaceView("Vault");
            setVaultError("");
            void refreshVault();
          }}>Open Vault<Icon name="arrow" /></button>
        </div>
      </dialog>
      <dialog ref={resetDialogRef} className="reset-dialog" aria-labelledby="reset-title" onCancel={() => { setResetError(""); }}>
        <form onSubmit={resetVault}>
          <h2 id="reset-title">Reset entire Vault?</h2>
          <p className="reset-warning">This permanently removes all saved people, identity mappings, custom rules, and document history. Pseudonymized documents may no longer be restorable through this Vault.</p>
          <p>Exported files and backups are separate and will remain on your computer.</p>
          {!vaultStatus.unlocked && vaultStatus.account_supported && <p>A locked Vault is replaced with an empty Vault protected by your current signed-in account.</p>}
          <label>Type RESET VAULT to confirm<input ref={resetConfirmationRef} required pattern="RESET VAULT" autoComplete="off" /></label>
          {resetError && <p className="error" role="alert">{resetError}</p>}
          <div className="data-actions"><button type="button" className="secondary-button" disabled={vaultBusy} onClick={() => { resetDialogRef.current?.close(); if (resetConfirmationRef.current) resetConfirmationRef.current.value = ""; setResetError(""); }}>Cancel</button><button className="danger-button" disabled={vaultBusy}>{vaultBusy ? "Resetting..." : "Permanently reset Vault"}</button></div>
        </form>
      </dialog>
    </div>
  );
}

export default App;
