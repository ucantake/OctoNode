import { useEffect, useState, type FormEvent, type ReactNode } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import clsx from "clsx";
import { ErrorText } from "./ErrorText";
import { Button, Field, inputClass, Modal } from "./Modal";
import { api, errorMessage, IpcError } from "../lib/ipc";
import { formatShortcut } from "../lib/platform";
import type { AccountStatus, AccountView, BootstrapState, PlatformInfo, SettingsView } from "../types/models";

type Section = "accounts" | "security" | "cloning" | "about";

const SECTIONS: Array<[Section, string]> = [
  ["accounts", "Accounts"],
  ["security", "Security"],
  ["cloning", "Cloning"],
  ["about", "About"],
];

export const SETTINGS_SHORTCUT = "Mod+,";

export interface SettingsDialogProps {
  platform: PlatformInfo;
  accounts: AccountView[];
  onEditAccount: (accountId: string) => void;
  onAddAccount: () => void;
  /** Current lock state from the app; settings re-read when it changes. */
  vaultLocked: boolean;
  /** Called with fresh app state after the vault was locked or unlocked. */
  onBootChanged: (state: BootstrapState) => void;
  /** Opens the unlock dialog (vault locked). */
  onRequestUnlock: () => void;
  onClose: () => void;
}

/**
 * Application settings. All changes apply immediately — including a master
 * password change, which re-encrypts the vault in place while it stays
 * unlocked.
 */
export function SettingsDialog(props: SettingsDialogProps) {
  const [section, setSection] = useState<Section>("accounts");
  const [settings, setSettings] = useState<SettingsView | null>(null);
  const [error, setError] = useState<string | null>(null);

  const reload = async () => {
    try {
      setSettings(await api.getSettings());
    } catch (e) {
      setError(errorMessage(e));
    }
  };
  useEffect(() => {
    void reload();
  }, [props.vaultLocked]);

  return (
    <Modal
      title="Settings"
      size="xl"
      onClose={props.onClose}
      footer={<Button onClick={props.onClose}>Done</Button>}
    >
      <div className="-mx-5 -my-4 flex min-h-[340px]">
        <nav className="w-40 shrink-0 border-r border-line py-3" aria-label="Settings sections">
          {SECTIONS.map(([key, label]) => (
            <button
              key={key}
              type="button"
              aria-current={section === key}
              onClick={() => setSection(key)}
              className={clsx(
                "block w-full px-4 py-1.5 text-left text-sm",
                section === key ? "bg-surface-3 text-fg" : "text-fg-muted hover:bg-surface-2 hover:text-fg",
              )}
            >
              {label}
            </button>
          ))}
          <p className="mt-6 px-4 text-[11px] text-fg-muted/70">{formatShortcut(SETTINGS_SHORTCUT)}</p>
        </nav>
        <div className="min-w-0 flex-1 space-y-4 px-5 py-4">
          {error && <ErrorText>{error}</ErrorText>}
          {!settings && !error && <p className="text-xs text-fg-muted">Loading…</p>}
          {section === "accounts" && (
            <AccountsSection accounts={props.accounts} onEdit={props.onEditAccount} onAdd={props.onAddAccount} />
          )}
          {settings && section === "security" && (
            <SecuritySection
              settings={settings}
              onChanged={(s) => setSettings(s)}
              onBootChanged={props.onBootChanged}
              onRequestUnlock={props.onRequestUnlock}
            />
          )}
          {settings && section === "cloning" && <CloningSection settings={settings} onSaved={setSettings} />}
          {settings && section === "about" && <AboutSection settings={settings} platform={props.platform} />}
        </div>
      </div>
    </Modal>
  );
}

function SectionTitle({ children, hint }: { children: ReactNode; hint?: string }) {
  return (
    <div>
      <h3 className="text-sm font-semibold text-fg">{children}</h3>
      {hint && <p className="mt-0.5 text-xs text-fg-muted">{hint}</p>}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Accounts
// ---------------------------------------------------------------------------

const HOST_LABEL: Record<AccountView["host"], string> = {
  gitHub: "GitHub",
  gitLabCloud: "GitLab.com",
  gitLabSelfHosted: "GitLab (self-hosted)",
  local: "Local",
};

const STATUS: Record<AccountStatus, [string, string]> = {
  ready: ["Token stored", "text-emerald-400"],
  missingToken: ["No token", "text-amber-300"],
  locked: ["Vault locked", "text-fg-muted"],
};

function AccountsSection(props: { accounts: AccountView[]; onEdit: (id: string) => void; onAdd: () => void }) {
  return (
    <>
      <SectionTitle hint="Saved connections: host, access token, SSH key and commit identity.">Accounts</SectionTitle>
      {props.accounts.length === 0 && <p className="text-xs text-fg-muted">No accounts yet.</p>}
      <ul className="divide-y divide-line rounded-md border border-line">
        {props.accounts.map((a) => {
          const [label, color] = a.host === "local" ? ["No token needed", "text-fg-muted"] : STATUS[a.status];
          return (
            <li key={a.id} className="flex items-center gap-3 px-3 py-2">
              <div className="min-w-0 flex-1">
                <div className="truncate text-sm text-fg">{a.label}</div>
                <div className="truncate text-[11px] text-fg-muted">
                  {HOST_LABEL[a.host]}
                  {a.host === "gitLabSelfHosted" && a.apiBaseUrl ? ` · ${a.apiBaseUrl}` : ""} · {a.identity.email}
                  {a.ssh.privateKeyPath ? " · SSH key" : a.ssh.useAgent ? " · ssh-agent" : ""}
                </div>
              </div>
              <span className={clsx("shrink-0 text-[11px]", color)}>{label}</span>
              <Button kind="ghost" onClick={() => props.onEdit(a.id)}>
                Edit
              </Button>
            </li>
          );
        })}
      </ul>
      <Button kind="ghost" onClick={props.onAdd}>
        + Add account
      </Button>
    </>
  );
}

// ---------------------------------------------------------------------------
// Security
// ---------------------------------------------------------------------------

function SecuritySection(props: {
  settings: SettingsView;
  onChanged: (s: SettingsView) => void;
  onBootChanged: (b: BootstrapState) => void;
  onRequestUnlock: () => void;
}) {
  const { vault } = props.settings;

  if (vault.backend === "osKeyring") {
    return (
      <>
        <SectionTitle hint="Access tokens are stored in your operating system's credential store.">
          Secret storage: system keychain
        </SectionTitle>
        <p className="rounded-md border border-line bg-surface-0 px-3 py-2 text-xs text-fg-muted">
          The keychain is protected by your OS login, so OctoNode has no separate master password. The
          encrypted-file vault (with a master password) is used only when no keychain is available.
        </p>
      </>
    );
  }

  const lockNow = async () => {
    const boot = await api.lockVault();
    props.onBootChanged(boot);
    props.onChanged({ ...props.settings, vault: boot.vault });
  };

  return (
    <>
      <SectionTitle hint="Tokens are encrypted with Argon2id + XChaCha20-Poly1305 under your master password.">
        Secret storage: encrypted file
      </SectionTitle>
      {vault.fallbackReason && (
        <p className="text-[11px] text-fg-muted/80" title={vault.fallbackReason}>
          Why not the system keychain: {vault.fallbackReason}
        </p>
      )}

      {vault.locked ? (
        <div className="flex items-center gap-3 rounded-md border border-line bg-surface-0 px-3 py-2 text-xs">
          <span className="text-fg-muted">The vault is locked. Unlock it to change the master password.</span>
          <Button kind="ghost" onClick={props.onRequestUnlock}>
            Unlock…
          </Button>
        </div>
      ) : (
        <>
          <ChangePasswordForm onChanged={(vaultStatus) => props.onChanged({ ...props.settings, vault: vaultStatus })} />
          <div className="flex items-center justify-between border-t border-line pt-3 text-xs">
            <span className="text-fg-muted">Remove the key from memory until you unlock again.</span>
            <Button kind="ghost" onClick={() => void lockNow()}>
              Lock vault now
            </Button>
          </div>
        </>
      )}
    </>
  );
}

const MIN_LENGTH = 8;

function strength(pw: string): { score: 0 | 1 | 2 | 3; label: string } {
  if (pw.length < MIN_LENGTH) return { score: 0, label: `At least ${MIN_LENGTH} characters` };
  const classes = [/[a-z]/, /[A-Z]/, /[0-9]/, /[^a-zA-Z0-9]/].filter((r) => r.test(pw)).length;
  if (pw.length >= 16 || (pw.length >= 12 && classes >= 3)) return { score: 3, label: "Strong" };
  if (pw.length >= 10 && classes >= 2) return { score: 2, label: "Fair" };
  return { score: 1, label: "Weak — consider a longer passphrase" };
}

function ChangePasswordForm(props: { onChanged: (vault: SettingsView["vault"]) => void }) {
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [confirm, setConfirm] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<{ ok: boolean; text: string } | null>(null);

  const s = strength(next);
  const mismatch = confirm.length > 0 && confirm !== next;
  const canSubmit = !busy && current.length > 0 && s.score > 0 && next === confirm;

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!canSubmit) return;
    setBusy(true);
    setMessage(null);
    try {
      const vault = await api.changeMasterPassword(current, next);
      props.onChanged(vault);
      setMessage({ ok: true, text: "Master password changed. It is in effect now — no restart needed." });
      setCurrent("");
      setNext("");
      setConfirm("");
    } catch (err) {
      const text =
        err instanceof IpcError && err.kind === "invalidPassword"
          ? "The current master password is incorrect."
          : errorMessage(err);
      setMessage({ ok: false, text });
    } finally {
      setBusy(false);
    }
  };

  return (
    <form onSubmit={submit} className="space-y-3" aria-label="Change master password">
      <Field label="Current master password">
        <input
          type="password"
          autoComplete="current-password"
          autoFocus
          className={inputClass}
          value={current}
          onChange={(e) => setCurrent(e.target.value)}
        />
      </Field>
      <div className="grid grid-cols-2 gap-3">
        <Field label="New master password">
          <input
            type="password"
            autoComplete="new-password"
            className={inputClass}
            value={next}
            onChange={(e) => setNext(e.target.value)}
          />
        </Field>
        <Field label="Confirm new password">
          <input
            type="password"
            autoComplete="new-password"
            className={clsx(inputClass, mismatch && "border-rose-500/70")}
            value={confirm}
            onChange={(e) => setConfirm(e.target.value)}
          />
        </Field>
      </div>
      {next.length > 0 && (
        <div className="flex items-center gap-2 text-[11px]">
          <div className="flex gap-1" aria-hidden>
            {[1, 2, 3].map((i) => (
              <span
                key={i}
                className={clsx(
                  "h-1 w-8 rounded",
                  s.score >= i
                    ? ["", "bg-rose-400", "bg-amber-400", "bg-emerald-400"][s.score]
                    : "bg-surface-3",
                )}
              />
            ))}
          </div>
          <span className="text-fg-muted">{s.label}</span>
          {mismatch && <span className="ml-auto text-rose-400">Passwords do not match</span>}
        </div>
      )}
      <div className="flex items-center gap-3">
        <Button type="submit" disabled={!canSubmit}>
          {busy ? "Re-encrypting…" : "Change master password"}
        </Button>
        {message &&
          (message.ok ? (
            <span role="status" className="text-xs text-emerald-400">
              {message.text}
            </span>
          ) : (
            <ErrorText className="min-w-0 flex-1">{message.text}</ErrorText>
          ))}
      </div>
    </form>
  );
}

// ---------------------------------------------------------------------------
// Cloning
// ---------------------------------------------------------------------------

function CloningSection(props: { settings: SettingsView; onSaved: (s: SettingsView) => void }) {
  const [dir, setDir] = useState(props.settings.cloneDirectory);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<{ ok: boolean; text: string } | null>(null);

  const save = async (value: string | null) => {
    setBusy(true);
    setMessage(null);
    try {
      const s = await api.updateSettings({ cloneDirectory: value });
      props.onSaved(s);
      setDir(s.cloneDirectory);
      setMessage({ ok: true, text: "Saved" });
    } catch (e) {
      setMessage({ ok: false, text: errorMessage(e) });
    } finally {
      setBusy(false);
    }
  };

  const browse = async () => {
    const picked = await open({ directory: true, multiple: false, title: "Default clone folder" });
    if (typeof picked === "string") setDir(picked);
  };

  return (
    <>
      <SectionTitle hint="New clones are created inside this folder unless you pick another one in the clone dialog.">
        Default clone folder
      </SectionTitle>
      <div className="flex gap-2">
        <input className={inputClass} value={dir} onChange={(e) => setDir(e.target.value)} spellCheck={false} />
        <Button kind="ghost" onClick={() => void browse()}>
          Browse…
        </Button>
      </div>
      <div className="flex items-center gap-2">
        <Button disabled={busy || dir === props.settings.cloneDirectory} onClick={() => void save(dir)}>
          Save
        </Button>
        {!props.settings.cloneDirectoryIsDefault && (
          <Button kind="ghost" disabled={busy} onClick={() => void save(null)}>
            Reset to default
          </Button>
        )}
        {message &&
          (message.ok ? (
            <span role="status" className="text-xs text-emerald-400">
              {message.text}
            </span>
          ) : (
            <ErrorText className="min-w-0 flex-1">{message.text}</ErrorText>
          ))}
      </div>
    </>
  );
}

// ---------------------------------------------------------------------------
// About
// ---------------------------------------------------------------------------

function AboutSection({ settings, platform }: { settings: SettingsView; platform: PlatformInfo }) {
  const rows: Array<[string, string]> = [
    ["Version", settings.appVersion],
    ["Platform", platform.os],
    ["Configuration", settings.configDir],
    ["Data (vault)", settings.dataDir],
  ];
  return (
    <>
      <SectionTitle>OctoNode</SectionTitle>
      <dl className="grid grid-cols-[8rem_1fr] gap-x-3 gap-y-1.5 text-xs">
        {rows.map(([k, v]) => (
          <div key={k} className="contents">
            <dt className="text-fg-muted">{k}</dt>
            <dd className="select-text break-all font-mono text-fg">{v}</dd>
          </div>
        ))}
      </dl>
    </>
  );
}
