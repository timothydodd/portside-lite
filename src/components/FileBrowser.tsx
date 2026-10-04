import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open as openDialog, save } from "@tauri-apps/plugin-dialog";
import {
  ArrowUp,
  ChevronRight,
  Download,
  File,
  Folder,
  FolderPlus,
  HardDrive,
  Link2,
  Lock,
  Pencil,
  RefreshCw,
  ShieldCheck,
  Trash2,
  Upload,
  X,
} from "lucide-react";
import * as ipc from "../lib/ipc";
import { confirmDestructive } from "../lib/dialog";
import { errorMessage, fmtAgo, fmtBytes, fmtDateTime } from "../lib/format";
import type { FileEntry, FileListing, FileProgress, FileSession } from "../lib/types";
import { confirmLeaveFiles, useFilesStore } from "../stores/files";
import { useNavStore } from "../stores/nav";
import { toast } from "../stores/toast";
import { ActionMenu, Drawer, EmptyState, Modal, Spinner } from "./ui";

const join = (dir: string, name: string) => (dir ? `${dir}/${name}` : name);
const isFolder = (e: FileEntry) => e.kind === "dir" || e.linkToDir;

/** Start (or join) the helper pod for a claim; closed again on unmount. */
function useFileSession(namespace: string, claim: string) {
  const [session, setSession] = useState<FileSession | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    let alive = true;
    let id: number | null = null;
    setSession(null);
    setError(null);
    ipc
      .openVolumeFiles(namespace, claim)
      .then((s) => {
        id = s.id;
        if (alive) setSession(s);
        else void ipc.closeVolumeFiles(s.id);
      })
      .catch((e) => alive && setError(errorMessage(e)));
    return () => {
      alive = false;
      if (id != null) void ipc.closeVolumeFiles(id);
    };
  }, [namespace, claim, attempt]);
  const refresh = useCallback(async () => {
    if (!session) return;
    try {
      setSession(await ipc.refreshVolumeFiles(session.id));
    } catch (e) {
      toast.error(errorMessage(e));
    }
  }, [session]);
  return { session, error, refresh, retry: () => setAttempt((n) => n + 1) };
}

/** Drawer around the browser, opened from the Storage page. */
export default function FilesDrawer() {
  const { files, closeFiles } = useNavStore();
  if (!files) return null;
  return (
    <Drawer
      title={files.claim}
      subtitle={<>PersistentVolumeClaim · {files.namespace} · files</>}
      wide
      explicitClose
      onClose={() => void confirmLeaveFiles().then((ok) => ok && closeFiles())}
    >
      <FileBrowser key={`${files.namespace}/${files.claim}`} namespace={files.namespace} claim={files.claim} />
    </Drawer>
  );
}

/**
 * Browse a PersistentVolumeClaim through a helper pod. Always readable;
 * writable only while nothing else mounts the claim (the backend re-checks
 * before every write).
 */
export function FileBrowser({ namespace, claim }: { namespace: string; claim: string }) {
  const { session, error, refresh, retry } = useFileSession(namespace, claim);
  const [path, setPath] = useState("");
  const [listing, setListing] = useState<FileListing | null>(null);
  const [listError, setListError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [transfer, setTransfer] = useState<FileProgress | null>(null);
  const [naming, setNaming] = useState<{ title: string; initial: string; submit: (name: string) => Promise<void> } | null>(null);
  const [dropping, setDropping] = useState(false);

  useEffect(() => setPath(""), [namespace, claim]);

  // Only the newest request may land: an answer for a folder the user already left is dropped.
  const loadSeq = useRef(0);
  const load = useCallback(async () => {
    if (!session) return;
    const seq = ++loadSeq.current;
    setLoading(true);
    try {
      const l = await ipc.listVolumeFiles(session.id, path);
      if (seq !== loadSeq.current) return;
      setListing(l);
      setListError(null);
    } catch (e) {
      if (seq === loadSeq.current) setListError(errorMessage(e));
    } finally {
      if (seq === loadSeq.current) setLoading(false);
    }
  }, [session?.id, session?.pod, path]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => void load(), [load]);

  const transferId = useRef<string | null>(null);
  useEffect(() => {
    const un = listen<FileProgress>("files:progress", (e) => {
      if (e.payload.transferId === transferId.current) setTransfer(e.payload);
    });
    return () => void un.then((f) => f());
  }, []);

  // Leaving mid-transfer (after the confirmation the drawers ask for) stops it.
  useEffect(
    () => () => {
      if (transferId.current) void ipc.cancelTransfer(transferId.current);
      useFilesStore.getState().setTransferring(false);
    },
    [],
  );

  const busy = transfer != null;
  // Rows belong to the folder they were listed from, which trails `path` while the next one loads.
  const shownPath = listing?.path ?? path;
  const writable = !!session?.writable;

  /** Run a write; a refusal (someone scaled the app back up) refreshes the session state. */
  const write = async (fn: () => Promise<void>) => {
    try {
      await fn();
    } catch (e) {
      const msg = errorMessage(e);
      toast.error(msg);
      if (msg.startsWith("Writes are off") || msg.startsWith("Read-only")) void refresh();
    }
    void load();
  };

  const runTransfer = async <T,>(label: string, fn: (id: string) => Promise<T>): Promise<T | null> => {
    const id = crypto.randomUUID();
    transferId.current = id;
    setTransfer({ transferId: id, label, done: 0, total: null });
    useFilesStore.getState().setTransferring(true);
    try {
      return await fn(id);
    } finally {
      // Unmounting cancels and clears these itself; a newer transfer owns them now.
      if (transferId.current === id) {
        transferId.current = null;
        setTransfer(null);
        useFilesStore.getState().setTransferring(false);
      }
    }
  };

  const download = async (e: FileEntry | null) => {
    if (!session) return;
    const folder = e == null || isFolder(e);
    const name = e?.name ?? (path.split("/").pop() || claim);
    const target = e ? join(shownPath, e.name) : path;
    const dest = await save({
      defaultPath: folder ? `${name}.tar.gz` : name,
      filters: folder ? [{ name: "gzip'd tar", extensions: ["tar.gz", "tgz"] }] : undefined,
    });
    if (!dest) return;
    try {
      const bytes = await runTransfer(name, (id) => ipc.downloadVolumePath(session.id, target, folder, dest, id));
      if (bytes != null) toast.success(`Saved ${name}${folder ? " (as .tar.gz)" : ""}, ${fmtBytes(bytes)}`);
    } catch (err) {
      toast.error(errorMessage(err));
    }
  };

  const upload = async (paths: string[]) => {
    if (!session || paths.length === 0) return;
    // A drop can arrive mid-transfer; a second one would take over the progress bar and the close guard.
    if (transferId.current) {
      toast.info("Wait for the current transfer to finish first.");
      return;
    }
    let picked;
    try {
      picked = await ipc.localPathInfo(paths);
    } catch (err) {
      toast.error(errorMessage(err));
      return;
    }
    // The overwrite check below needs this folder's own rows.
    if (listing?.path !== path) {
      toast.info("Still loading this folder; try again in a moment.");
      return;
    }
    const existing = new Map(listing.entries.map((x) => [x.name, x]));
    const clashes = picked.filter((p) => existing.has(p.name));
    if (clashes.length > 0) {
      const badFolder = clashes.find((c) => !c.dir && existing.get(c.name) && isFolder(existing.get(c.name)!));
      if (badFolder) {
        toast.error(`There's already a folder named ${badFolder.name} here.`);
        return;
      }
      const lines = clashes.map((c) => (c.dir ? `• ${c.name}/ (merged: files with the same names inside are replaced)` : `• ${c.name}`));
      const ok = await confirmDestructive(
        `These already exist in /${path} and will be replaced:\n\n${lines.join("\n")}`,
        "Replace files",
      );
      if (!ok) return;
    }
    await write(async () => {
      const r = await runTransfer(picked.length === 1 ? picked[0].name : `${picked.length} items`, (id) =>
        ipc.uploadVolumeFiles(session.id, path, paths, id),
      );
      if (r) toast.success(`Uploaded ${r.files} file${r.files === 1 ? "" : "s"} (${fmtBytes(r.bytes)})`);
    });
  };

  // Drag files in from Explorer (Tauri hands over the local paths).
  const uploadRef = useRef(upload);
  uploadRef.current = upload;
  useEffect(() => {
    if (!writable) return;
    let un: (() => void) | undefined;
    let cancelled = false;
    try {
      void getCurrentWebview()
        .onDragDropEvent((e) => {
          if (e.payload.type === "enter") setDropping(true);
          else if (e.payload.type === "leave") setDropping(false);
          else if (e.payload.type === "drop") {
            setDropping(false);
            void uploadRef.current(e.payload.paths);
          }
        })
        .then((f) => (cancelled ? f() : (un = f)));
    } catch {
      // Not running inside Tauri (browser preview).
    }
    return () => {
      cancelled = true;
      un?.();
      setDropping(false);
    };
  }, [writable]);

  const pick = async (directory: boolean) => {
    const r = await openDialog({ multiple: true, directory });
    const paths = r == null ? [] : Array.isArray(r) ? r : [r];
    await upload(paths);
  };

  const remove = async (e: FileEntry) => {
    if (!session) return;
    const what = isFolder(e) && e.kind === "dir" ? `the folder ${e.name} and everything in it` : e.name;
    if (!(await confirmDestructive(`Delete ${what} from ${claim}?\n\nThis can't be undone.`, "Delete"))) return;
    await write(async () => {
      await ipc.deleteVolumePath(session.id, join(shownPath, e.name));
      toast.success(`Deleted ${e.name}`);
    });
  };

  const crumbs = useMemo(() => (path ? path.split("/") : []), [path]);

  if (error) {
    return (
      <EmptyState icon={<HardDrive size={28} />} title="Couldn't open the volume">
        <p className="mb-3 whitespace-pre-wrap">{error}</p>
        <button className="btn-ghost" onClick={retry}>
          <RefreshCw size={14} /> Try again
        </button>
      </EmptyState>
    );
  }
  if (!session) {
    return (
      <EmptyState icon={<Spinner size={24} />} title="Starting the file browser…">
        A small pod that mounts {claim} is starting in {namespace}. The first time can take a little longer while its image is
        pulled.
      </EmptyState>
    );
  }

  return (
    <div className="relative flex h-full flex-col">
      <AccessBanner session={session} onRecheck={refresh} />

      <div className="flex shrink-0 flex-wrap items-center gap-2 border-b border-border-light px-4 py-2">
        <button className="btn-quiet" disabled={!path} title="Up one folder" onClick={() => setPath(crumbs.slice(0, -1).join("/"))}>
          <ArrowUp size={14} />
        </button>
        <nav className="flex min-w-0 flex-1 items-center gap-0.5 text-sm">
          <button className="truncate rounded px-1 text-content-secondary hover:text-content" onClick={() => setPath("")}>
            {claim}
          </button>
          {crumbs.map((c, i) => (
            <span key={i} className="flex min-w-0 items-center gap-0.5">
              <ChevronRight size={12} className="shrink-0 text-content-muted" />
              <button
                className={`truncate rounded px-1 ${i === crumbs.length - 1 ? "text-content" : "text-content-secondary hover:text-content"}`}
                onClick={() => setPath(crumbs.slice(0, i + 1).join("/"))}
              >
                {c}
              </button>
            </span>
          ))}
        </nav>
        {loading && <Spinner size={14} />}
        <button className="btn-quiet" title="Refresh" onClick={() => void load()}>
          <RefreshCw size={14} />
        </button>
        <button className="btn-ghost" disabled={busy} title="Download this folder as .tar.gz" onClick={() => void download(null)}>
          <Download size={14} /> Folder
        </button>
        {writable && (
          <>
            <button
              className="btn-ghost"
              disabled={busy}
              onClick={() =>
                setNaming({
                  title: "New folder",
                  initial: "",
                  submit: (name) => write(() => ipc.makeVolumeDir(session.id, path, name)),
                })
              }
            >
              <FolderPlus size={14} /> New folder
            </button>
            <button className="btn-ghost" disabled={busy} title="Upload a folder with its contents" onClick={() => void pick(true)}>
              <Upload size={14} /> Folder
            </button>
            <button className="btn-primary" disabled={busy} onClick={() => void pick(false)}>
              <Upload size={14} /> Upload files
            </button>
          </>
        )}
      </div>

      <div className="min-h-0 flex-1 overflow-auto">
        {listError ? (
          <EmptyState title="Couldn't read this folder">
            <p className="mb-3">{listError}</p>
            <div className="flex justify-center gap-2">
              {path && (
                <button className="btn-ghost" onClick={() => setPath("")}>
                  Back to the top
                </button>
              )}
              <button className="btn-ghost" onClick={() => void refresh().then(load)}>
                <RefreshCw size={14} /> Reconnect
              </button>
            </div>
          </EmptyState>
        ) : listing == null ? (
          <EmptyState icon={<Spinner />} title="Loading…" />
        ) : listing.entries.length === 0 ? (
          <EmptyState icon={<Folder size={24} />} title="Empty folder">
            {writable ? "Upload files, or drag them here from your computer." : null}
          </EmptyState>
        ) : (
          <table className="table">
            <thead>
              <tr>
                <th>Name</th>
                <th className="text-right">Size</th>
                <th className="text-right">Modified</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {listing.entries.map((e) => {
                const folder = isFolder(e);
                const Icon = e.kind === "link" ? Link2 : folder ? Folder : File;
                return (
                  <tr
                    key={e.name}
                    className={folder ? "cursor-pointer" : undefined}
                    onClick={folder ? () => setPath(join(shownPath, e.name)) : undefined}
                  >
                    <td>
                      <span className="flex items-center gap-2">
                        <Icon size={14} className={folder ? "shrink-0 text-accent" : "shrink-0 text-content-muted"} />
                        <span className="mono truncate text-content" title={e.name}>{e.name}</span>
                      </span>
                    </td>
                    <td className="whitespace-nowrap text-right tabular-nums text-content-secondary">
                      {e.kind === "file" ? fmtBytes(e.size) : "—"}
                    </td>
                    <td className="whitespace-nowrap text-right text-xs text-content-muted" title={e.modifiedMs ? fmtDateTime(e.modifiedMs) : undefined}>
                      {e.modifiedMs ? fmtAgo(e.modifiedMs) : "—"}
                    </td>
                    <td className="whitespace-nowrap text-right" onClick={(ev) => ev.stopPropagation()}>
                      <ActionMenu
                        label={`Actions for ${e.name}`}
                        items={[
                          (e.kind === "file" || folder) && {
                            label: folder ? "Download as .tar.gz" : "Download",
                            icon: <Download size={14} />,
                            disabled: busy,
                            onSelect: () => void download(e),
                          },
                          writable && {
                            label: "Rename",
                            icon: <Pencil size={14} />,
                            disabled: busy,
                            onSelect: () =>
                              setNaming({
                                title: `Rename ${e.name}`,
                                initial: e.name,
                                submit: (name) => write(() => ipc.renameVolumePath(session.id, join(shownPath, e.name), name)),
                              }),
                          },
                          writable && { label: "Delete", icon: <Trash2 size={14} />, danger: true, disabled: busy, onSelect: () => void remove(e) },
                        ]}
                      />
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}
      </div>

      <div className="flex shrink-0 items-center gap-3 border-t border-border-light px-4 py-2 text-xs text-content-muted">
        {transfer ? (
          <TransferBar transfer={transfer} />
        ) : (
          <>
            <span className="flex-1">
              {listing && `${listing.entries.length} item${listing.entries.length === 1 ? "" : "s"}`}
              {listing?.freeBytes != null && listing.totalBytes != null && ` · ${fmtBytes(listing.freeBytes)} free of ${fmtBytes(listing.totalBytes)}`}
            </span>
            <span className="mono" title={`Helper pod, stops on its own ${fmtDateTime(session.expiresMs)}`}>
              {session.pod}
            </span>
          </>
        )}
      </div>

      {dropping && (
        <div className="pointer-events-none absolute inset-2 flex items-center justify-center rounded-lg border-2 border-dashed border-accent bg-surface/80 text-sm text-content">
          <Upload size={16} className="mr-2 text-accent" /> Drop to upload into /{path}
        </div>
      )}
      {naming && <NameDialog {...naming} onClose={() => setNaming(null)} />}
    </div>
  );
}

function AccessBanner({ session, onRecheck }: { session: FileSession; onRecheck: () => Promise<void> }) {
  const [checking, setChecking] = useState(false);
  const recheck = async () => {
    setChecking(true);
    await onRecheck();
    setChecking(false);
  };
  if (session.writable) {
    return (
      <div className="tint-good flex shrink-0 items-start gap-2 px-4 py-2 text-xs">
        <ShieldCheck size={14} className="mt-0.5 shrink-0" />
        <span className="flex-1">
          <b>Writable.</b> Nothing else is using this volume. While this browser is open the volume stays attached to its helper
          pod, so close it before scaling the app back up.
        </span>
      </div>
    );
  }
  return (
    <div className="tint-muted flex shrink-0 items-start gap-2 px-4 py-2 text-xs">
      <Lock size={14} className="mt-0.5 shrink-0" />
      <div className="flex-1">
        <b>Read-only.</b> Files can be changed only while no app uses the volume:
        <ul className="mt-1 list-disc pl-4">
          {session.blockers.map((b) => (
            <li key={b}>{b}</li>
          ))}
        </ul>
      </div>
      <button className="btn-chip shrink-0" disabled={checking} onClick={() => void recheck()}>
        {checking ? <Spinner size={12} /> : <RefreshCw size={12} />} Check again
      </button>
    </div>
  );
}

function TransferBar({ transfer }: { transfer: FileProgress }) {
  const pct = transfer.total ? Math.min(100, (transfer.done / transfer.total) * 100) : null;
  return (
    <>
      <Spinner size={12} />
      <span className="mono max-w-[40%] truncate text-content-secondary" title={transfer.label}>{transfer.label}</span>
      <div className="h-1.5 flex-1 overflow-hidden rounded bg-muted">
        <div
          className={`h-full bg-accent ${pct == null ? "w-1/3 animate-pulse" : ""}`}
          style={pct != null ? { width: `${pct}%` } : undefined}
        />
      </div>
      <span className="tabular-nums">
        {fmtBytes(transfer.done)}
        {transfer.total != null && ` of ${fmtBytes(transfer.total)}`}
      </span>
      <button className="btn-quiet" title="Cancel" onClick={() => void ipc.cancelTransfer(transfer.transferId)}>
        <X size={14} />
      </button>
    </>
  );
}

function NameDialog({
  title,
  initial,
  submit,
  onClose,
}: {
  title: string;
  initial: string;
  submit: (name: string) => Promise<void>;
  onClose: () => void;
}) {
  const [name, setName] = useState(initial);
  const invalid = !name.trim() || name === "." || name === ".." || /[/\\]/.test(name);
  return (
    <Modal title={title} onClose={onClose}>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          if (invalid || name === initial) return;
          onClose();
          void submit(name.trim());
        }}
      >
        <input autoFocus className="field w-full" value={name} onChange={(e) => setName(e.target.value)} placeholder="Name" />
        <div className="mt-4 flex justify-end gap-2">
          <button type="button" className="btn-ghost" onClick={onClose}>
            Cancel
          </button>
          <button type="submit" className="btn-primary" disabled={invalid || name === initial}>
            Save
          </button>
        </div>
      </form>
    </Modal>
  );
}
