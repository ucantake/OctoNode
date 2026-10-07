import { useEffect, useMemo, useRef, useState, type FormEvent, type ReactNode } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import clsx from "clsx";
import { ErrorAction, ErrorText } from "./ErrorText";
import { Button, Field, inputClass, Modal } from "./Modal";
import { api, errorMessage, IpcError, onCloneProgress } from "../lib/ipc";
import {
  accountHost,
  cloneProgressFraction,
  describeProgress,
  folderNameFromUrl,
  isHttpsUrl,
  newCloneId,
  urlHost,
} from "../lib/clone";
import { relativeTime } from "../lib/theme";
import type { AccountView, CloneProgress, CloneResult, RemoteRepo, Workspace } from "../types/models";

type Source = "account" | "url";
type Protocol = "https" | "ssh";

const HOST_NAME: Record<AccountView["host"], string> = {
  gitHub: "GitHub",
  gitLabCloud: "GitLab",
  gitLabSelfHosted: "GitLab",
  local: "Local",
};

export interface CloneDialogProps {
  workspace: Workspace;
  /** The workspace's account; credentials and identity come from it. */
  account: AccountView | null;
  onCloned: (result: CloneResult) => void;
  /** Opens the account editor (e.g. to add a missing token). */
  onEditAccount: () => void;
  onClose: () => void;
}

/**
 * Clone a remote repository into a workspace: pick one from the account's
 * GitHub/GitLab repositories, or paste any URL. The clone runs with the
 * workspace account's SSH key / token, reports progress and can be cancelled.
 */
export function CloneDialog({ workspace, account, onCloned, onEditAccount, onClose }: CloneDialogProps) {
  const hasApi = account !== null && account.host !== "local";
  // Listing needs the account's token; without it, cloning by URL still works
  // (SSH key / agent, or public HTTPS).
  const canBrowse = hasApi && account.status === "ready";
  const [source, setSource] = useState<Source>(canBrowse ? "account" : "url");
  // An explicit SSH key on the account → SSH; otherwise HTTPS uses the token.
  const [protocol, setProtocol] = useState<Protocol>(account?.ssh.privateKeyPath ? "ssh" : "https");

  const [repos, setRepos] = useState<RemoteRepo[] | null>(null);
  const [reposError, setReposError] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<RemoteRepo | null>(null);

  const [url, setUrl] = useState("");
  const [parent, setParent] = useState("");
  const [folder, setFolder] = useState("");
  const [folderTouched, setFolderTouched] = useState(false);
  const [bindIdentity, setBindIdentity] = useState(true);

  const [cloneId, setCloneId] = useState<string | null>(null);
  const [progress, setProgress] = useState<CloneProgress | null>(null);
  const [error, setError] = useState<{ authRequired: boolean; text: string } | null>(null);
  const running = cloneId !== null;

  // Default parent folder from settings.
  useEffect(() => {
    api
      .getSettings()
      .then((s) => setParent((p) => p || s.cloneDirectory))
      .catch(() => undefined);
  }, []);

  // Account repositories.
  useEffect(() => {
    if (!canBrowse || !account) return;
    let alive = true;
    api
      .listRemoteRepositories(account.id)
      .then((r) => alive && setRepos(r))
      .catch((e: unknown) => {
        if (!alive) return;
        setReposError(errorMessage(e));
        setSource("url");
      });
    return () => {
      alive = false;
    };
  }, [canBrowse, account]);

  // The effective URL comes from the selected repo (account tab) or the field.
  const effectiveUrl =
    source === "account" ? (selected ? (protocol === "ssh" ? selected.sshUrl : selected.httpsUrl) : "") : url.trim();

  useEffect(() => {
    if (!folderTouched) setFolder(folderNameFromUrl(effectiveUrl) ?? "");
  }, [effectiveUrl, folderTouched]);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!repos) return [];
    return q
      ? repos.filter((r) => r.fullName.toLowerCase().includes(q) || r.description?.toLowerCase().includes(q))
      : repos;
  }, [repos, query]);

  // Ensure the progress subscription is dropped if the dialog unmounts.
  const unlisten = useRef<(() => void) | null>(null);
  useEffect(() => () => unlisten.current?.(), []);

  const browseParent = async () => {
    const picked = await open({ directory: true, multiple: false, title: "Clone into folder", defaultPath: parent });
    if (typeof picked === "string") setParent(picked);
  };

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!effectiveUrl || running) return;
    const id = newCloneId();
    setError(null);
    setProgress(null);
    setCloneId(id);
    // Subscribe before starting so no early event is missed.
    unlisten.current = await onCloneProgress(id, setProgress);
    try {
      const result = await api.cloneRepository({
        cloneId: id,
        workspaceId: workspace.id,
        url: effectiveUrl,
        parentDirectory: parent.trim() || null,
        folderName: folder.trim() || null,
        bindIdentity: bindIdentity && account !== null,
      });
      onCloned(result);
    } catch (err) {
      setError({
        authRequired: err instanceof IpcError && err.kind === "authRequired",
        text: err instanceof IpcError && err.kind === "cancelled" ? "Clone cancelled." : errorMessage(err),
      });
    } finally {
      unlisten.current?.();
      unlisten.current = null;
      setCloneId(null);
    }
  };

  const cancel = () => {
    if (cloneId) void api.cancelClone(cloneId);
  };

  // Warn before cloning when HTTPS auth can't possibly work.
  const needsTokenWarning =
    account !== null &&
    account.status !== "ready" &&
    isHttpsUrl(effectiveUrl) &&
    urlHost(effectiveUrl) !== null &&
    urlHost(effectiveUrl) === accountHost(account);
  const editAction = account ? <ErrorAction onClick={onEditAccount}>Edit account</ErrorAction> : undefined;

  const sep = parent.includes("\\") && !parent.includes("/") ? "\\" : "/";
  const destination = parent && folder ? `${parent.replace(/[\\/]+$/, "")}${sep}${folder}` : "";
  const fraction = progress ? cloneProgressFraction(progress) : null;

  return (
    <Modal
      title={`Clone into “${workspace.name}”`}
      size="xl"
      onClose={running ? undefined : onClose}
      footer={
        <>
          {running ? (
            <Button kind="ghost" onClick={cancel}>
              Cancel clone
            </Button>
          ) : (
            <Button kind="ghost" onClick={onClose}>
              Close
            </Button>
          )}
          <Button type="submit" form="clone-form" disabled={running || !effectiveUrl || !folder.trim() || !parent.trim()}>
            {running ? "Cloning…" : "Clone"}
          </Button>
        </>
      }
    >
      <form id="clone-form" onSubmit={submit} className="space-y-3">
        <div className="flex items-center gap-1 text-xs" role="tablist">
          {canBrowse && account && (
            <TabButton active={source === "account"} onClick={() => setSource("account")} disabled={running}>
              From {HOST_NAME[account.host]}
            </TabButton>
          )}
          <TabButton active={source === "url"} onClick={() => setSource("url")} disabled={running}>
            By URL
          </TabButton>
          <span className="ml-auto text-fg-muted">
            {account ? `as ${account.label} · ${account.identity.email}` : "with your global git configuration"}
          </span>
        </div>

        {source === "account" && canBrowse && account && (
          <div className="space-y-2">
            <div className="flex gap-2">
              <input
                className={inputClass}
                placeholder="Filter repositories…"
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                disabled={running}
              />
              <ProtocolToggle value={protocol} onChange={setProtocol} disabled={running} />
            </div>
            <div className="h-56 overflow-y-auto rounded-md border border-line bg-surface-0" role="listbox">
              {!repos && !reposError && <p className="p-3 text-xs text-fg-muted">Loading repositories…</p>}
              {repos && filtered.length === 0 && <p className="p-3 text-xs text-fg-muted">No matching repositories.</p>}
              {filtered.map((r) => (
                <button
                  key={r.fullName}
                  type="button"
                  role="option"
                  aria-selected={selected?.fullName === r.fullName}
                  disabled={running}
                  onClick={() => setSelected(r)}
                  className={clsx(
                    "flex w-full items-center gap-2 border-b border-line/50 px-3 py-1.5 text-left",
                    selected?.fullName === r.fullName ? "bg-accent/20" : "hover:bg-surface-2",
                  )}
                >
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-sm text-fg">
                      <span className="text-fg-muted">{r.namespace}/</span>
                      {r.name}
                    </span>
                    {r.description && <span className="block truncate text-[11px] text-fg-muted">{r.description}</span>}
                  </span>
                  {r.private && (
                    <span className="shrink-0 rounded border border-line px-1 text-[10px] text-fg-muted">private</span>
                  )}
                  {r.updatedAt && (
                    <span className="w-24 shrink-0 text-right text-[11px] text-fg-muted">
                      {relativeTime(Date.parse(r.updatedAt) / 1000)}
                    </span>
                  )}
                </button>
              ))}
            </div>
          </div>
        )}

        {source === "url" && (
          <>
            {reposError && (
              <ErrorText tone="warning" action={editAction}>
                Could not list repositories: {reposError}
              </ErrorText>
            )}
            {hasApi && account && account.status !== "ready" && (
              <p className="text-[11px] text-fg-muted">
                {account.status === "locked"
                  ? "Unlock the secret vault to browse this account's repositories."
                  : `Add an access token to ${account.label} to browse its repositories.`}
              </p>
            )}
            <Field label="Repository URL" hint="https://…, ssh://… or git@host:group/repo.git">
              <input
                className={clsx(inputClass, "font-mono")}
                placeholder="git@github.com:owner/repo.git"
                value={url}
                onChange={(e) => setUrl(e.target.value)}
                spellCheck={false}
                autoFocus
                disabled={running}
              />
            </Field>
          </>
        )}

        <div className="grid grid-cols-[1fr_12rem] gap-3">
          <Field label="Parent folder">
            <div className="flex gap-2">
              <input
                className={inputClass}
                value={parent}
                onChange={(e) => setParent(e.target.value)}
                spellCheck={false}
                disabled={running}
              />
              <Button kind="ghost" onClick={() => void browseParent()}>
                Browse…
              </Button>
            </div>
          </Field>
          <Field label="Folder name">
            <input
              className={inputClass}
              value={folder}
              onChange={(e) => {
                setFolderTouched(true);
                setFolder(e.target.value);
              }}
              spellCheck={false}
              disabled={running}
            />
          </Field>
        </div>
        {destination && <p className="truncate font-mono text-[11px] text-fg-muted" title={destination}>→ {destination}</p>}

        {account && (
          <label className="flex items-center gap-2 text-xs text-fg-muted">
            <input
              type="checkbox"
              checked={bindIdentity}
              onChange={(e) => setBindIdentity(e.target.checked)}
              disabled={running}
            />
            Write {account.identity.email}
            {account.ssh.privateKeyPath ? " and the account's SSH key" : ""} into the clone's .git/config, so terminal
            git uses the same identity
          </label>
        )}

        {(running || progress) && (
          <div className="space-y-1" aria-live="polite">
            <div className="h-1.5 overflow-hidden rounded bg-surface-3">
              {fraction === null ? (
                // Separate element: never animates its width into the real bar.
                <div key="indeterminate" className="h-full w-1/3 animate-pulse bg-accent" />
              ) : (
                <div
                  key="determinate"
                  className="h-full bg-accent transition-[width] duration-150"
                  style={{ width: `${Math.round(fraction * 100)}%` }}
                />
              )}
            </div>
            <p className="text-[11px] text-fg-muted">{progress ? describeProgress(progress) : "Connecting…"}</p>
          </div>
        )}
        {needsTokenWarning && !error && (
          <ErrorText tone="warning" action={editAction}>
            {account?.status === "locked"
              ? "The secret vault is locked, so this account's token can't be used. Private repositories over HTTPS will fail until you unlock it."
              : `\u201c${account?.label}\u201d has no access token. Public repositories clone fine, but private ones over HTTPS need a token \u2014 add one, or use the SSH URL.`}
          </ErrorText>
        )}
        {error && <ErrorText action={error.authRequired ? editAction : undefined}>{error.text}</ErrorText>}
      </form>
    </Modal>
  );
}

function TabButton(props: { active: boolean; disabled?: boolean; onClick: () => void; children: ReactNode }) {
  return (
    <button
      type="button"
      role="tab"
      aria-selected={props.active}
      disabled={props.disabled}
      onClick={props.onClick}
      className={clsx(
        "rounded-md px-2.5 py-1",
        props.active ? "bg-surface-3 text-fg" : "text-fg-muted hover:text-fg",
      )}
    >
      {props.children}
    </button>
  );
}

function ProtocolToggle(props: { value: Protocol; onChange: (p: Protocol) => void; disabled?: boolean }) {
  return (
    <div className="flex shrink-0 overflow-hidden rounded-md border border-line text-xs" role="radiogroup" aria-label="Protocol">
      {(["https", "ssh"] as const).map((p) => (
        <button
          key={p}
          type="button"
          role="radio"
          aria-checked={props.value === p}
          disabled={props.disabled}
          onClick={() => props.onChange(p)}
          className={clsx("px-2.5 uppercase", props.value === p ? "bg-surface-3 text-fg" : "text-fg-muted hover:text-fg")}
        >
          {p}
        </button>
      ))}
    </div>
  );
}
