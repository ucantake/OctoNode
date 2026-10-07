import { useMemo, useState, type ReactNode } from "react";
import clsx from "clsx";
import { Avatar } from "./Avatar";
import { formatShortcut } from "../lib/platform";
import type { AccountStatus, AccountView, GitHostType, RepositoryRef, Uuid, Workspace } from "../types/models";

/** Account rail key: an account id, or `LOCAL_KEY` for unbound workspaces. */
export const LOCAL_KEY = "local";
export type AccountKey = Uuid | typeof LOCAL_KEY;

export interface WorkspaceSidebarProps {
  accounts: AccountView[];
  workspaces: Workspace[];
  selectedAccount: AccountKey;
  activeWorkspaceId: Uuid | null;
  selectedRepoId: Uuid | null;
  onSelectAccount: (key: AccountKey) => void;
  onSelectRepo: (workspaceId: Uuid, repoId: Uuid) => void;
  onAddRepository: (workspaceId: Uuid) => void;
  onCloneRepository: (workspaceId: Uuid) => void;
  onOpenSettings: () => void;
  onCreateWorkspace: (accountId: Uuid | null) => void;
  onAddAccount: () => void;
}

const HOST_LABEL: Record<GitHostType, string> = {
  gitHub: "GitHub",
  gitLabCloud: "GitLab",
  gitLabSelfHosted: "GitLab (self-hosted)",
  local: "Local",
};

const HOST_GLYPH: Record<GitHostType, string> = {
  gitHub: "GH",
  gitLabCloud: "GL",
  gitLabSelfHosted: "SH",
  local: "LO",
};

const STATUS_STYLE: Record<AccountStatus, { dot: string; label: string }> = {
  ready: { dot: "bg-emerald-400", label: "Connected" },
  missingToken: { dot: "bg-amber-400", label: "No access token" },
  locked: { dot: "bg-zinc-500", label: "Vault locked" },
};

/**
 * Two-pane switcher: a compact account rail (Slack/Linear style) and a tree of
 * the selected account's workspaces → organizations → repositories.
 */
export function WorkspaceSidebar(props: WorkspaceSidebarProps) {
  const {
    accounts,
    workspaces,
    selectedAccount,
    activeWorkspaceId,
    selectedRepoId,
    onSelectAccount,
    onSelectRepo,
    onAddRepository,
    onCloneRepository,
    onOpenSettings,
    onCreateWorkspace,
    onAddAccount,
  } = props;

  const visibleWorkspaces = useMemo(
    () =>
      workspaces.filter((w) =>
        selectedAccount === LOCAL_KEY ? w.accountId === null : w.accountId === selectedAccount,
      ),
    [workspaces, selectedAccount],
  );
  const current = accounts.find((a) => a.id === selectedAccount) ?? null;

  return (
    <aside className="flex h-full shrink-0 border-r border-line bg-surface-0 text-sm">
      {/* ---- Account rail -------------------------------------------------- */}
      <nav aria-label="Accounts" className="flex w-14 flex-col items-center gap-2 border-r border-line py-3">
        {accounts.map((account, i) => (
          <AccountButton
            key={account.id}
            account={account}
            active={account.id === selectedAccount}
            shortcut={i < 9 ? formatShortcut(`Mod+${i + 1}`) : undefined}
            onClick={() => onSelectAccount(account.id)}
          />
        ))}
        <RailButton
          active={selectedAccount === LOCAL_KEY}
          title="Local repositories"
          onClick={() => onSelectAccount(LOCAL_KEY)}
        >
          <span className="text-[11px] font-semibold text-fg-muted">LO</span>
        </RailButton>
        <div className="mt-auto" />
        <RailButton title="Add account" onClick={onAddAccount}>
          <span className="text-lg leading-none text-fg-muted">+</span>
        </RailButton>
        <RailButton title={`Settings (${formatShortcut("Mod+,")})`} onClick={onOpenSettings}>
          <svg viewBox="0 0 20 20" className="h-4 w-4 text-fg-muted" aria-hidden>
            <path
              fill="currentColor"
              d="M11.1 1.5l.4 2.1c.5.2 1 .5 1.4.8l2-.8 1.1 1.9-1.6 1.4c.1.5.1 1.1 0 1.6l1.6 1.4-1.1 1.9-2-.8c-.4.3-.9.6-1.4.8l-.4 2.1H8.9l-.4-2.1c-.5-.2-1-.5-1.4-.8l-2 .8-1.1-1.9 1.6-1.4a5 5 0 010-1.6L4 5.5l1.1-1.9 2 .8c.4-.3.9-.6 1.4-.8l.4-2.1h2.2zM10 7a2 2 0 100 4 2 2 0 000-4z"
              transform="translate(0 1.5)"
            />
          </svg>
        </RailButton>
      </nav>

      {/* ---- Tree ---------------------------------------------------------- */}
      <div className="flex w-64 min-w-0 flex-col">
        <header className="flex items-center gap-2 border-b border-line px-3 py-3">
          <div className="min-w-0 flex-1">
            <div className="truncate font-medium text-fg">{current?.label ?? "Local"}</div>
            <div className="truncate text-xs text-fg-muted">
              {current ? `${HOST_LABEL[current.host]} · ${current.identity.email}` : "Your git config & ssh-agent"}
            </div>
          </div>
          <button
            type="button"
            className="rounded px-1.5 py-0.5 text-fg-muted hover:bg-surface-2 hover:text-fg"
            title="New workspace"
            onClick={() => onCreateWorkspace(current?.id ?? null)}
          >
            +
          </button>
        </header>

        <div role="tree" aria-label="Workspaces" className="flex-1 overflow-y-auto py-2">
          {visibleWorkspaces.length === 0 && (
            <p className="px-3 py-6 text-center text-xs text-fg-muted">
              No workspaces yet.
              <br />
              Create one with “+”, then open or clone repositories into it.
            </p>
          )}
          {visibleWorkspaces.map((ws) => (
            <WorkspaceNode
              key={ws.id}
              workspace={ws}
              active={ws.id === activeWorkspaceId}
              selectedRepoId={selectedRepoId}
              onSelectRepo={(repoId) => onSelectRepo(ws.id, repoId)}
              onAddRepository={() => onAddRepository(ws.id)}
              onCloneRepository={() => onCloneRepository(ws.id)}
            />
          ))}
        </div>
      </div>
    </aside>
  );
}

function RailButton(props: {
  active?: boolean;
  title: string;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      title={props.title}
      aria-label={props.title}
      aria-pressed={props.active}
      onClick={props.onClick}
      className={clsx(
        "relative flex h-9 w-9 items-center justify-center rounded-xl transition-colors",
        props.active ? "bg-surface-3 ring-1 ring-accent/60" : "hover:bg-surface-2",
      )}
    >
      {props.active && <span className="absolute -left-3 h-5 w-1 rounded-r bg-accent" />}
      {props.children}
    </button>
  );
}

function AccountButton(props: {
  account: AccountView;
  active: boolean;
  shortcut?: string;
  onClick: () => void;
}) {
  const { account } = props;
  const status = STATUS_STYLE[account.status];
  const title = [account.label, HOST_LABEL[account.host], status.label, props.shortcut]
    .filter(Boolean)
    .join(" · ");
  return (
    <RailButton active={props.active} title={title} onClick={props.onClick}>
      <Avatar
        name={account.label}
        seed={account.color ?? account.identity.email}
        url={account.avatarUrl}
        size={28}
        className="rounded-lg"
      />
      <span
        className="absolute -bottom-0.5 -right-0.5 rounded bg-surface-0 px-0.5 text-[8px] font-bold leading-tight text-fg-muted"
        aria-hidden
      >
        {HOST_GLYPH[account.host]}
      </span>
      <span
        className={clsx("absolute -right-0.5 -top-0.5 h-2.5 w-2.5 rounded-full ring-2 ring-surface-0", status.dot)}
        aria-hidden
      />
    </RailButton>
  );
}

function WorkspaceNode(props: {
  workspace: Workspace;
  active: boolean;
  selectedRepoId: Uuid | null;
  onSelectRepo: (repoId: Uuid) => void;
  onAddRepository: () => void;
  onCloneRepository: () => void;
}) {
  const { workspace } = props;
  const [expanded, setExpanded] = useState(true);
  const [collapsedOrgs, setCollapsedOrgs] = useState<ReadonlySet<string>>(new Set());

  const groups = useMemo(() => groupByOrganization(workspace.repositories), [workspace.repositories]);

  const toggleOrg = (org: string) =>
    setCollapsedOrgs((prev) => {
      const next = new Set(prev);
      if (next.has(org)) next.delete(org);
      else next.add(org);
      return next;
    });

  return (
    <div role="treeitem" aria-expanded={expanded} aria-selected={props.active} className="mb-1">
      <div className="group flex items-center px-2">
        <button
          type="button"
          onClick={() => setExpanded((v) => !v)}
          className={clsx(
            "flex min-w-0 flex-1 items-center gap-1.5 rounded px-1 py-1 text-left text-xs font-semibold uppercase tracking-wide",
            props.active ? "text-fg" : "text-fg-muted hover:text-fg",
          )}
        >
          <Chevron open={expanded} />
          <span className="truncate">{workspace.name}</span>
        </button>
        <button
          type="button"
          title="Clone a remote repository"
          aria-label="Clone a remote repository"
          onClick={props.onCloneRepository}
          className="rounded px-1 text-fg-muted opacity-0 hover:bg-surface-2 hover:text-fg group-hover:opacity-100 focus:opacity-100"
        >
          <svg viewBox="0 0 16 16" className="h-3.5 w-3.5" aria-hidden>
            <path d="M8 2v8m0 0l-3-3m3 3l3-3M3 13h10" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
          </svg>
        </button>
        <button
          type="button"
          title="Open a local repository"
          aria-label="Open a local repository"
          onClick={props.onAddRepository}
          className="rounded px-1.5 text-fg-muted opacity-0 hover:bg-surface-2 hover:text-fg group-hover:opacity-100 focus:opacity-100"
        >
          +
        </button>
      </div>

      {expanded && (
        <div role="group">
          {groups.map(([org, repos]) => {
            const open = !collapsedOrgs.has(org);
            return (
              <div key={org} role="treeitem" aria-expanded={open} aria-selected={false}>
                <button
                  type="button"
                  onClick={() => toggleOrg(org)}
                  className="flex w-full items-center gap-1.5 px-4 py-1 text-left text-xs text-fg-muted hover:text-fg"
                >
                  <Chevron open={open} />
                  <span className="truncate">{org}</span>
                  <span className="ml-auto pr-1 tabular-nums">{repos.length}</span>
                </button>
                {open && (
                  <ul role="group">
                    {repos.map((repo) => (
                      <li key={repo.id} role="treeitem" aria-selected={repo.id === props.selectedRepoId}>
                        <button
                          type="button"
                          title={repo.path}
                          onClick={() => props.onSelectRepo(repo.id)}
                          className={clsx(
                            "flex w-full items-center gap-2 py-1 pl-9 pr-3 text-left",
                            repo.id === props.selectedRepoId
                              ? "bg-accent/15 text-fg"
                              : "text-fg-muted hover:bg-surface-2 hover:text-fg",
                          )}
                        >
                          <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-current opacity-60" />
                          <span className="truncate">{repo.name}</span>
                        </button>
                      </li>
                    ))}
                  </ul>
                )}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

function Chevron({ open }: { open: boolean }) {
  return (
    <svg
      viewBox="0 0 16 16"
      className={clsx("h-3 w-3 shrink-0 transition-transform", open && "rotate-90")}
      aria-hidden
    >
      <path d="M6 4l4 4-4 4" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
    </svg>
  );
}

function groupByOrganization(repos: RepositoryRef[]): Array<[string, RepositoryRef[]]> {
  const map = new Map<string, RepositoryRef[]>();
  for (const repo of repos) {
    const key = repo.organization ?? "Local";
    const list = map.get(key);
    if (list) list.push(repo);
    else map.set(key, [repo]);
  }
  for (const list of map.values()) list.sort((a, b) => a.name.localeCompare(b.name));
  return [...map.entries()].sort(([a], [b]) => a.localeCompare(b));
}
