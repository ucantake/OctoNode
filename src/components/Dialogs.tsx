import { useState, type FormEvent } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { ErrorText } from "./ErrorText";
import { Button, Field, inputClass, Modal } from "./Modal";
import { api, errorMessage } from "../lib/ipc";
import type { AccountInput, AccountView, BootstrapState, GitHostType, VaultStatus } from "../types/models";

// ---------------------------------------------------------------------------
// Vault unlock (encrypted-file fallback only)
// ---------------------------------------------------------------------------

export function VaultUnlockDialog(props: {
  vault: VaultStatus;
  onUnlocked: (state: BootstrapState) => void;
  onSkip: () => void;
}) {
  const creating = !props.vault.initialized;
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (creating && password !== confirm) {
      setError("Passwords do not match");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      props.onUnlocked(await api.unlockVault(password));
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
      setPassword("");
      setConfirm("");
    }
  };

  return (
    <Modal
      title={creating ? "Create a master password" : "Unlock secret vault"}
      onClose={props.onSkip}
      footer={
        <>
          <Button kind="ghost" onClick={props.onSkip}>
            Later
          </Button>
          <Button type="submit" form="vault-form" disabled={busy || password.length === 0}>
            {busy ? "Deriving key…" : creating ? "Create vault" : "Unlock"}
          </Button>
        </>
      }
    >
      <p className="text-xs text-fg-muted">
        The system keychain is unavailable
        {props.vault.fallbackReason ? ` (${props.vault.fallbackReason})` : ""}. Tokens are stored in a local file
        encrypted with Argon2id + XChaCha20-Poly1305 instead.
      </p>
      <form id="vault-form" onSubmit={submit} className="space-y-3">
        <Field label="Master password">
          <input
            type="password"
            autoComplete={creating ? "new-password" : "current-password"}
            className={inputClass}
            value={password}
            onChange={(e) => setPassword(e.target.value)}
          />
        </Field>
        {creating && (
          <Field label="Confirm password" hint="At least 8 characters. It cannot be recovered.">
            <input
              type="password"
              autoComplete="new-password"
              className={inputClass}
              value={confirm}
              onChange={(e) => setConfirm(e.target.value)}
            />
          </Field>
        )}
      </form>
      {error && <ErrorText>{error}</ErrorText>}
    </Modal>
  );
}

// ---------------------------------------------------------------------------
// Account
// ---------------------------------------------------------------------------

const HOSTS: Array<[GitHostType, string]> = [
  ["gitHub", "GitHub"],
  ["gitLabCloud", "GitLab.com"],
  ["gitLabSelfHosted", "GitLab (self-hosted)"],
  ["local", "Local only"],
];

type SecretEdit = "keep" | "replace" | "remove";

export interface AccountDialogProps {
  /** Present = edit this saved account; absent = create a new one. */
  account?: AccountView;
  onSaved: (a: AccountView) => void;
  onDeleted?: (id: string) => void;
  onClose: () => void;
}

const EMPTY_ACCOUNT: AccountInput = {
  label: "",
  host: "gitHub",
  apiBaseUrl: null,
  username: "",
  avatarUrl: null,
  identity: { name: "", email: "" },
  ssh: { privateKeyPath: null, useAgent: true },
  color: null,
};

/** Create or edit a saved account (connection), including its secrets. */
export function AccountDialog({ account, onSaved, onDeleted, onClose }: AccountDialogProps) {
  const editing = account !== undefined;
  const [form, setForm] = useState<AccountInput>(() => {
    if (!account) return EMPTY_ACCOUNT;
    const { id: _id, status: _status, ...input } = account;
    return input;
  });
  const hasStoredToken = account?.status === "ready" && account.host !== "local";
  const [tokenEdit, setTokenEdit] = useState<SecretEdit>(hasStoredToken ? "keep" : "replace");
  const [token, setToken] = useState("");
  const [passphrase, setPassphrase] = useState("");
  const [clearPassphrase, setClearPassphrase] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [test, setTest] = useState<{ ok: boolean; text: string } | null>(null);

  const set = <K extends keyof AccountInput>(key: K, value: AccountInput[K]) =>
    setForm((f) => ({ ...f, [key]: value }));

  const pickKey = async () => {
    const path = await open({ multiple: false, directory: false, title: "Select SSH private key" });
    if (typeof path === "string") set("ssh", { ...form.ssh, privateKeyPath: path });
  };

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const username = form.username.trim();
      const input: AccountInput = {
        ...form,
        apiBaseUrl: form.host === "gitLabSelfHosted" ? form.apiBaseUrl : null,
        // GitHub serves avatars by username; GitLab needs an API call (later).
        avatarUrl:
          form.host === "gitHub" && username
            ? `https://avatars.githubusercontent.com/${encodeURIComponent(username)}?s=64`
            : null,
      };
      let saved = account ? await api.updateAccount(account.id, input) : await api.createAccount(input);

      // Secrets are written separately and only when changed.
      if (form.host !== "local") {
        if (tokenEdit === "replace" && token.trim()) {
          saved = await api.setAccountSecret(saved.id, "token", token);
        } else if (tokenEdit === "remove") {
          saved = await api.setAccountSecret(saved.id, "token", null);
        }
      }
      if (passphrase) {
        saved = await api.setAccountSecret(saved.id, "sshPassphrase", passphrase);
      } else if (clearPassphrase) {
        saved = await api.setAccountSecret(saved.id, "sshPassphrase", null);
      }
      onSaved(saved);
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  const testToken = async () => {
    if (!account) return;
    setTest(null);
    try {
      const repos = await api.listRemoteRepositories(account.id);
      setTest({ ok: true, text: `Token works: ${repos.length} repositories visible.` });
    } catch (err) {
      setTest({ ok: false, text: errorMessage(err) });
    }
  };

  const remove = async () => {
    if (!account) return;
    setBusy(true);
    try {
      await api.deleteAccount(account.id);
      onDeleted?.(account.id);
    } catch (err) {
      setError(errorMessage(err));
      setBusy(false);
    }
  };

  const tokenDirty = tokenEdit !== "keep";

  return (
    <Modal
      title={editing ? `Edit account \u201c${account.label}\u201d` : "Add account"}
      size="lg"
      onClose={onClose}
      footer={
        <>
          {editing && (
            <div className="mr-auto">
              {confirmDelete ? (
                <span className="flex items-center gap-2 text-xs text-rose-300">
                  Delete this account and its stored secrets? Its workspaces stay, unbound.
                  <Button kind="ghost" onClick={() => setConfirmDelete(false)}>
                    Keep
                  </Button>
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() => void remove()}
                    className="rounded-md bg-rose-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-rose-500"
                  >
                    Delete
                  </button>
                </span>
              ) : (
                <Button kind="ghost" onClick={() => setConfirmDelete(true)}>
                  Delete account…
                </Button>
              )}
            </div>
          )}
          {!confirmDelete && (
            <>
              <Button kind="ghost" onClick={onClose}>
                Cancel
              </Button>
              <Button type="submit" form="account-form" disabled={busy}>
                {busy ? "Saving…" : editing ? "Save changes" : "Add account"}
              </Button>
            </>
          )}
        </>
      }
    >
      <form id="account-form" onSubmit={submit} className="grid grid-cols-2 gap-3">
        <Field label="Label">
          <input
            className={inputClass}
            placeholder="Work GitLab"
            value={form.label}
            onChange={(e) => set("label", e.target.value)}
          />
        </Field>
        <Field label="Host">
          <select className={inputClass} value={form.host} onChange={(e) => set("host", e.target.value as GitHostType)}>
            {HOSTS.map(([v, l]) => (
              <option key={v} value={v}>
                {l}
              </option>
            ))}
          </select>
        </Field>
        {form.host === "gitLabSelfHosted" && (
          <div className="col-span-2">
            <Field label="API base URL" hint="e.g. https://git.example.com/api/v4">
              <input
                className={inputClass}
                value={form.apiBaseUrl ?? ""}
                onChange={(e) => set("apiBaseUrl", e.target.value)}
              />
            </Field>
          </div>
        )}
        <Field label="Username">
          <input className={inputClass} value={form.username} onChange={(e) => set("username", e.target.value)} />
        </Field>
        {form.host !== "local" ? (
          <Field
            label="Access token"
            hint={
              form.host === "gitHub"
                ? "Needed for private repos over HTTPS and for browsing. Scope: repo."
                : "Needed for private repos over HTTPS and for browsing. Scopes: read_api, read_repository (write_repository to push)."
            }
          >
            {tokenEdit === "keep" ? (
              <div className="flex items-center gap-2">
                <span className="flex-1 truncate rounded-md border border-line bg-surface-0 px-2.5 py-1.5 text-sm text-emerald-300">
                  ●●●●●●●● stored
                </span>
                <Button kind="ghost" onClick={() => setTokenEdit("replace")}>
                  Replace
                </Button>
                <Button kind="ghost" onClick={() => setTokenEdit("remove")}>
                  Remove
                </Button>
              </div>
            ) : tokenEdit === "remove" ? (
              <div className="flex items-center gap-2">
                <span className="flex-1 text-xs text-amber-300">The stored token will be deleted on save.</span>
                <Button kind="ghost" onClick={() => setTokenEdit("keep")}>
                  Undo
                </Button>
              </div>
            ) : (
              <div className="flex items-center gap-2">
                <input
                  type="password"
                  autoComplete="off"
                  className={inputClass}
                  placeholder={hasStoredToken ? "New token" : "Paste a personal access token"}
                  value={token}
                  onChange={(e) => setToken(e.target.value)}
                />
                {hasStoredToken && (
                  <Button kind="ghost" onClick={() => setTokenEdit("keep")}>
                    Undo
                  </Button>
                )}
              </div>
            )}
          </Field>
        ) : (
          <div />
        )}
        <Field label="Author name">
          <input
            className={inputClass}
            value={form.identity.name}
            onChange={(e) => set("identity", { ...form.identity, name: e.target.value })}
          />
        </Field>
        <Field label="Author email">
          <input
            type="email"
            className={inputClass}
            value={form.identity.email}
            onChange={(e) => set("identity", { ...form.identity, email: e.target.value })}
          />
        </Field>
        <div className="col-span-2">
          <Field label="SSH private key" hint="Pinned with IdentitiesOnly so other keys in your agent are never offered.">
            <div className="flex gap-2">
              <input
                className={inputClass}
                value={form.ssh.privateKeyPath ?? ""}
                placeholder={form.ssh.useAgent ? "Use ssh-agent" : "No SSH key"}
                spellCheck={false}
                onChange={(e) => set("ssh", { ...form.ssh, privateKeyPath: e.target.value || null })}
              />
              <Button kind="ghost" onClick={() => void pickKey()}>
                Browse…
              </Button>
            </div>
          </Field>
        </div>
        <Field label="SSH key passphrase" hint={editing ? "Leave empty to keep the stored one." : "Only for encrypted keys."}>
          <input
            type="password"
            autoComplete="off"
            className={inputClass}
            value={passphrase}
            onChange={(e) => {
              setPassphrase(e.target.value);
              if (e.target.value) setClearPassphrase(false);
            }}
          />
        </Field>
        <div className="flex flex-col justify-end gap-1.5 pb-1 text-xs text-fg-muted">
          <label className="flex items-center gap-2">
            <input
              type="checkbox"
              checked={form.ssh.useAgent}
              onChange={(e) => set("ssh", { ...form.ssh, useAgent: e.target.checked })}
            />
            Fall back to ssh-agent when no key is set
          </label>
          {editing && (
            <label className="flex items-center gap-2">
              <input
                type="checkbox"
                checked={clearPassphrase}
                disabled={passphrase.length > 0}
                onChange={(e) => setClearPassphrase(e.target.checked)}
              />
              Remove stored passphrase
            </label>
          )}
        </div>
      </form>

      {editing && form.host !== "local" && (
        <div className="flex items-center gap-2">
          <Button kind="ghost" disabled={tokenDirty || !hasStoredToken} onClick={() => void testToken()}>
            Test token
          </Button>
          {tokenDirty && <span className="text-[11px] text-fg-muted">Save first to test the new token.</span>}
          {!tokenDirty && !hasStoredToken && <span className="text-[11px] text-fg-muted">No token stored.</span>}
          {test && (
            <span className="min-w-0 flex-1">
              {test.ok ? (
                <span className="text-xs text-emerald-400">{test.text}</span>
              ) : (
                <ErrorText>{test.text}</ErrorText>
              )}
            </span>
          )}
        </div>
      )}
      {error && <ErrorText>{error}</ErrorText>}
    </Modal>
  );
}

// ---------------------------------------------------------------------------
// Simple text prompt (workspace name)
// ---------------------------------------------------------------------------

export function PromptDialog(props: {
  title: string;
  label: string;
  confirmLabel: string;
  onSubmit: (value: string) => Promise<void>;
  onClose: () => void;
}) {
  const [value, setValue] = useState("");
  const [error, setError] = useState<string | null>(null);
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    try {
      await props.onSubmit(value);
    } catch (err) {
      setError(errorMessage(err));
    }
  };
  return (
    <Modal
      title={props.title}
      onClose={props.onClose}
      footer={
        <>
          <Button kind="ghost" onClick={props.onClose}>
            Cancel
          </Button>
          <Button type="submit" form="prompt-form" disabled={!value.trim()}>
            {props.confirmLabel}
          </Button>
        </>
      }
    >
      <form id="prompt-form" onSubmit={submit}>
        <Field label={props.label}>
          <input className={inputClass} value={value} onChange={(e) => setValue(e.target.value)} />
        </Field>
      </form>
      {error && <ErrorText>{error}</ErrorText>}
    </Modal>
  );
}
