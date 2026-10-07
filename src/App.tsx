import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import clsx from "clsx";
import { CommitGraphView } from "./components/CommitGraphView";
import { AccountDialog, PromptDialog, VaultUnlockDialog } from "./components/Dialogs";
import { DiffViewer } from "./components/DiffViewer";
import { LOCAL_KEY, WorkspaceSidebar, type AccountKey } from "./components/WorkspaceSidebar";
import { useCommitGraph } from "./hooks/useCommitGraph";
import { useHotkeys } from "./hooks/useHotkeys";
import { api, errorMessage, IpcError } from "./lib/ipc";
import { formatShortcut, setPlatform } from "./lib/platform";
import type { BootstrapState, CommitNode, DiffTarget, Uuid } from "./types/models";

type Tab = "history" | "changes";
type DialogState = null | { type: "account" } | { type: "workspace"; accountId: Uuid | null };

const WORKING_TREE: DiffTarget = { type: "workingTree" };
const INDEX: DiffTarget = { type: "index" };

export default function App() {
  const [boot, setBoot] = useState<BootstrapState | null>(null);
  const [fatal, setFatal] = useState<string | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const [vaultSkipped, setVaultSkipped] = useState(false);
  const [dialog, setDialog] = useState<DialogState>(null);

  const [selectedAccount, setSelectedAccount] = useState<AccountKey>(LOCAL_KEY);
  const [repoId, setRepoId] = useState<Uuid | null>(null);
  const [tab, setTab] = useState<Tab>("history");
  const [selectedCommit, setSelectedCommit] = useState<CommitNode | null>(null);
  const [changesToken, setChangesToken] = useState(0);
  const [fetching, setFetching] = useState(false);

  const applyBoot = useCallback((state: BootstrapState) => {
    setPlatform(state.platform);
    setBoot(state);
  }, []);

  const reload = useCallback(async () => {
    try {
      applyBoot(await api.initWorkspace());
    } catch (e) {
      setFatal(errorMessage(e));
    }
  }, [applyBoot]);

  useEffect(() => {
    void reload();
  }, [reload]);

  // On first load, the account rail follows the persisted active workspace.
  const initialized = useRef(false);
  useEffect(() => {
    if (!boot || initialized.current) return;
    initialized.current = true;
    const active = boot.workspaces.find((w) => w.id === boot.activeWorkspaceId);
    if (active?.accountId) setSelectedAccount(active.accountId);
  }, [boot]);

  const graph = useCommitGraph(tab === "history" ? repoId : null);

  const repo = useMemo(() => {
    for (const ws of boot?.workspaces ?? []) {
      const r = ws.repositories.find((x) => x.id === repoId);
      if (r) return { repo: r, workspace: ws };
    }
    return null;
  }, [boot, repoId]);
  const account = boot?.accounts.find((a) => a.id === repo?.workspace.accountId) ?? null;

  const notify = (msg: string) => {
    setToast(msg);
    window.setTimeout(() => setToast((t) => (t === msg ? null : t)), 5000);
  };

  const selectRepo = async (workspaceId: Uuid, id: Uuid) => {
    setRepoId(id);
    setSelectedCommit(null);
    if (boot?.activeWorkspaceId !== workspaceId) {
      try {
        await api.setActiveWorkspace(workspaceId);
        setBoot((b) => (b ? { ...b, activeWorkspaceId: workspaceId } : b));
      } catch (e) {
        notify(errorMessage(e));
      }
    }
  };

  const addRepository = async (workspaceId: Uuid) => {
    const path = await open({ directory: true, multiple: false, title: "Open repository" });
    if (typeof path !== "string") return;
    try {
      const repoRef = await api.addRepository(workspaceId, path);
      await reload();
      await selectRepo(workspaceId, repoRef.id);
    } catch (e) {
      notify(errorMessage(e));
    }
  };

  const fetchRepo = async () => {
    if (!repoId) return;
    setFetching(true);
    try {
      const res = await api.fetchRemote(repoId, "origin");
      notify(`Fetched ${res.remote} via ${res.transport === "libgit2" ? "libgit2" : "git CLI"}`);
      graph.refresh();
    } catch (e) {
      if (e instanceof IpcError && e.kind === "vaultLocked") setVaultSkipped(false);
      notify(errorMessage(e));
    } finally {
      setFetching(false);
    }
  };

  // ---- Shortcuts (Mod = ⌘ on macOS, Ctrl elsewhere) ---------------------
  const hotkeys: Record<string, () => void> = {
    "Alt+1": () => setTab("history"),
    "Alt+2": () => setTab("changes"),
    "Mod+Shift+F": () => void fetchRepo(),
    "Mod+R": () => (tab === "history" ? graph.refresh() : setChangesToken((t) => t + 1)),
  };
  boot?.accounts.slice(0, 9).forEach((a, i) => {
    hotkeys[`Mod+${i + 1}`] = () => setSelectedAccount(a.id);
  });
  useHotkeys(hotkeys, dialog === null);

  if (fatal) {
    return (
      <div className="flex h-full items-center justify-center p-8 text-sm text-rose-300">
        Failed to start: {fatal}
      </div>
    );
  }
  if (!boot) {
    return <div className="flex h-full items-center justify-center text-sm text-fg-muted">Loading…</div>;
  }

  const showVault = boot.vault.backend === "encryptedFile" && boot.vault.locked && !vaultSkipped;

  return (
    <div className="flex h-full bg-surface-0 text-fg">
      <WorkspaceSidebar
        accounts={boot.accounts}
        workspaces={boot.workspaces}
        selectedAccount={selectedAccount}
        activeWorkspaceId={boot.activeWorkspaceId}
        selectedRepoId={repoId}
        onSelectAccount={setSelectedAccount}
        onSelectRepo={(ws, id) => void selectRepo(ws, id)}
        onAddRepository={(ws) => void addRepository(ws)}
        onCreateWorkspace={(accountId) => setDialog({ type: "workspace", accountId })}
        onAddAccount={() => setDialog({ type: "account" })}
      />

      <main className="flex min-w-0 flex-1 flex-col">
        <header className="flex h-12 shrink-0 items-center gap-4 border-b border-line px-4">
          <div className="min-w-0">
            <div className="truncate text-sm font-semibold">{repo?.repo.name ?? "No repository selected"}</div>
            <div className="truncate text-[11px] text-fg-muted">
              {account
                ? `Committing as ${account.identity.name} <${account.identity.email}>`
                : repo
                  ? "Using your global git identity"
                  : boot.configDir}
            </div>
          </div>
          {repo && (
            <nav className="ml-4 flex gap-1 text-sm" role="tablist">
              {(["history", "changes"] as const).map((t, i) => (
                <button
                  key={t}
                  role="tab"
                  type="button"
                  aria-selected={tab === t}
                  title={formatShortcut(`Alt+${i + 1}`)}
                  onClick={() => setTab(t)}
                  className={clsx(
                    "rounded-md px-3 py-1 capitalize",
                    tab === t ? "bg-surface-3 text-fg" : "text-fg-muted hover:text-fg",
                  )}
                >
                  {t}
                </button>
              ))}
            </nav>
          )}
          <div className="ml-auto flex items-center gap-2">
            {repo && (
              <button
                type="button"
                onClick={() => void fetchRepo()}
                disabled={fetching}
                title={`Fetch origin (${formatShortcut("Mod+Shift+F")})`}
                className="rounded-md border border-line px-3 py-1 text-xs text-fg-muted hover:border-accent/60 hover:text-fg disabled:opacity-50"
              >
                {fetching ? "Fetching…" : "Fetch"}
              </button>
            )}
            {boot.vault.backend === "encryptedFile" && (
              <span
                className="rounded bg-surface-2 px-2 py-0.5 text-[11px] text-fg-muted"
                title={boot.vault.fallbackReason ?? undefined}
              >
                {boot.vault.locked ? "Vault locked" : "File vault"}
              </span>
            )}
          </div>
        </header>

        {!repo && (
          <div className="flex flex-1 items-center justify-center text-sm text-fg-muted">
            Select a repository, or create a workspace and add one.
          </div>
        )}

        {repo && tab === "history" && (
          <div className="flex min-h-0 flex-1 flex-col">
            <div className="min-h-0 flex-[3] border-b border-line">
              <CommitGraphView source={graph} selectedId={selectedCommit?.id ?? null} onSelect={setSelectedCommit} />
            </div>
            <div className="min-h-0 flex-[2]">
              {selectedCommit ? (
                <DiffViewer repoId={repo.repo.id} target={{ type: "commit", id: selectedCommit.id }} />
              ) : (
                <div className="flex h-full items-center justify-center text-sm text-fg-muted">
                  Select a commit to see its changes
                </div>
              )}
            </div>
          </div>
        )}

        {repo && tab === "changes" && (
          <div className="grid min-h-0 flex-1 grid-rows-2">
            <div className="flex min-h-0 flex-col border-b border-line">
              <h2 className="px-3 pt-2 text-[11px] font-semibold uppercase tracking-wide text-fg-muted">Unstaged</h2>
              <div className="min-h-0 flex-1">
                <DiffViewer
                  repoId={repo.repo.id}
                  target={WORKING_TREE}
                  reloadToken={changesToken}
                  onChanged={() => setChangesToken((t) => t + 1)}
                />
              </div>
            </div>
            <div className="flex min-h-0 flex-col">
              <h2 className="px-3 pt-2 text-[11px] font-semibold uppercase tracking-wide text-fg-muted">Staged</h2>
              <div className="min-h-0 flex-1">
                <DiffViewer
                  repoId={repo.repo.id}
                  target={INDEX}
                  reloadToken={changesToken}
                  onChanged={() => setChangesToken((t) => t + 1)}
                />
              </div>
            </div>
          </div>
        )}
      </main>

      {toast && (
        <div className="fixed bottom-4 right-4 z-40 max-w-md rounded-lg border border-line bg-surface-2 px-4 py-2 text-sm shadow-xl">
          {toast}
        </div>
      )}

      {showVault && (
        <VaultUnlockDialog vault={boot.vault} onUnlocked={applyBoot} onSkip={() => setVaultSkipped(true)} />
      )}
      {dialog?.type === "account" && (
        <AccountDialog
          onClose={() => setDialog(null)}
          onCreated={(a) => {
            setDialog(null);
            setSelectedAccount(a.id);
            void reload();
          }}
        />
      )}
      {dialog?.type === "workspace" && (
        <PromptDialog
          title="New workspace"
          label="Name"
          confirmLabel="Create"
          onClose={() => setDialog(null)}
          onSubmit={async (name) => {
            await api.createWorkspace(name, dialog.accountId);
            setDialog(null);
            await reload();
          }}
        />
      )}
    </div>
  );
}
