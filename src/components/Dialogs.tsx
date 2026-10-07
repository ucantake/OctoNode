import { useState, type FormEvent } from "react";
import { open } from "@tauri-apps/plugin-dialog";
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
      {error && <p className="text-xs text-rose-400">{error}</p>}
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

export function AccountDialog(props: { onCreated: (a: AccountView) => void; onClose: () => void }) {
  const [form, setForm] = useState<AccountInput>({
    label: "",
    host: "gitHub",
    apiBaseUrl: null,
    username: "",
    avatarUrl: null,
    identity: { name: "", email: "" },
    ssh: { privateKeyPath: null, useAgent: true },
    color: null,
  });
  const [token, setToken] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

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
      const input: AccountInput = {
        ...form,
        apiBaseUrl: form.host === "gitLabSelfHosted" ? form.apiBaseUrl : null,
        // GitHub serves avatars by username; GitLab needs an API call (later).
        avatarUrl:
          form.host === "gitHub" && form.username.trim()
            ? `https://avatars.githubusercontent.com/${encodeURIComponent(form.username.trim())}?s=64`
            : null,
      };
      let account = await api.createAccount(input);
      if (token.trim() && form.host !== "local") {
        account = await api.setAccountSecret(account.id, "token", token);
      }
      props.onCreated(account);
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      title="Add account"
      size="lg"
      onClose={props.onClose}
      footer={
        <>
          <Button kind="ghost" onClick={props.onClose}>
            Cancel
          </Button>
          <Button type="submit" form="account-form" disabled={busy}>
            {busy ? "Saving…" : "Add account"}
          </Button>
        </>
      }
    >
      <form id="account-form" onSubmit={submit} className="grid grid-cols-2 gap-3">
        <Field label="Label">
          <input className={inputClass} placeholder="Work GitLab" value={form.label} onChange={(e) => set("label", e.target.value)} />
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
          <Field label="Access token">
            <input
              type="password"
              autoComplete="off"
              className={inputClass}
              value={token}
              onChange={(e) => setToken(e.target.value)}
            />
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
                placeholder="Use ssh-agent"
                onChange={(e) => set("ssh", { ...form.ssh, privateKeyPath: e.target.value || null })}
              />
              <Button kind="ghost" onClick={() => void pickKey()}>
                Browse…
              </Button>
            </div>
          </Field>
        </div>
      </form>
      {error && <p className="text-xs text-rose-400">{error}</p>}
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
      {error && <p className="text-xs text-rose-400">{error}</p>}
    </Modal>
  );
}
