// Typed IPC layer. Components never call `invoke` directly: every backend
// command has exactly one typed wrapper here, and every rejection is
// normalized into `IpcError`.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AccountInput,
  AccountView,
  AppErrorKind,
  AppErrorPayload,
  BootstrapState,
  CloneProgress,
  CloneRequest,
  CloneResult,
  DiffResult,
  DiffTarget,
  FetchOutcome,
  GraphPage,
  RemoteRepo,
  RepositoryRef,
  SettingsInput,
  SettingsView,
  StageRequest,
  VaultStatus,
  Uuid,
  Workspace,
} from "../types/models";

export class IpcError extends Error {
  readonly kind: AppErrorKind;
  readonly command: string;

  constructor(command: string, payload: AppErrorPayload) {
    super(payload.message);
    this.name = "IpcError";
    this.kind = payload.kind;
    this.command = command;
  }
}

function isAppErrorPayload(value: unknown): value is AppErrorPayload {
  return (
    typeof value === "object" &&
    value !== null &&
    typeof (value as Record<string, unknown>).kind === "string" &&
    typeof (value as Record<string, unknown>).message === "string"
  );
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (raw) {
    if (isAppErrorPayload(raw)) throw new IpcError(command, raw);
    // Tauri-level failures (unknown command, bad args) arrive as strings.
    throw new IpcError(command, { kind: "internal", message: String(raw) });
  }
}

export const api = {
  initWorkspace: () => call<BootstrapState>("init_workspace"),
  unlockVault: (masterPassword: string) =>
    call<BootstrapState>("unlock_vault", { masterPassword }),
  lockVault: () => call<BootstrapState>("lock_vault"),

  setActiveWorkspace: (workspaceId: Uuid) => call<void>("set_active_workspace", { workspaceId }),
  createWorkspace: (name: string, accountId: Uuid | null) =>
    call<Workspace>("create_workspace", { name, accountId }),
  deleteWorkspace: (workspaceId: Uuid) => call<void>("delete_workspace", { workspaceId }),
  addRepository: (workspaceId: Uuid, path: string) =>
    call<RepositoryRef>("add_repository", { workspaceId, path }),
  removeRepository: (workspaceId: Uuid, repoId: Uuid) =>
    call<void>("remove_repository", { workspaceId, repoId }),

  createAccount: (input: AccountInput) => call<AccountView>("create_account", { input }),
  updateAccount: (accountId: Uuid, input: AccountInput) =>
    call<AccountView>("update_account", { accountId, input }),
  deleteAccount: (accountId: Uuid) => call<void>("delete_account", { accountId }),
  setAccountSecret: (accountId: Uuid, kind: "token" | "sshPassphrase", value: string | null) =>
    call<AccountView>("set_account_secret", { accountId, kind, value }),
  bindRepositoryIdentity: (repoId: Uuid) => call<void>("bind_repository_identity", { repoId }),

  getCommitGraph: (repoId: Uuid, offset: number, limit: number) =>
    call<GraphPage>("get_commit_graph", { repoId, offset, limit }),
  getDiff: (repoId: Uuid, target: DiffTarget, contextLines?: number, ignoreWhitespace?: boolean) =>
    call<DiffResult>("get_diff", { repoId, target, contextLines, ignoreWhitespace }),
  stageChanges: (request: StageRequest) => call<void>("stage_changes", { request }),
  getSettings: () => call<SettingsView>("get_settings"),
  updateSettings: (input: SettingsInput) => call<SettingsView>("update_settings", { input }),
  changeMasterPassword: (currentPassword: string, newPassword: string) =>
    call<VaultStatus>("change_master_password", { currentPassword, newPassword }),

  listRemoteRepositories: (accountId: Uuid) =>
    call<RemoteRepo[]>("list_remote_repositories", { accountId }),
  cloneRepository: (request: CloneRequest) => call<CloneResult>("clone_repository", { request }),
  cancelClone: (cloneId: Uuid) => call<boolean>("cancel_clone", { cloneId }),

  openRepositoryFolder: (repoId: Uuid) => call<void>("open_repository_folder", { repoId }),
  fetchRemote: (repoId: Uuid, remote: string) =>
    call<FetchOutcome>("fetch_remote", { repoId, remote }),
} as const;

export function errorMessage(e: unknown): string {
  if (e instanceof Error) return e.message;
  return String(e);
}

/** Subscribes to clone progress for one clone; returns the unsubscribe fn. */
export function onCloneProgress(
  cloneId: Uuid,
  handler: (progress: CloneProgress) => void,
): Promise<UnlistenFn> {
  return listen<CloneProgress>("clone-progress", (event) => {
    if (event.payload.cloneId === cloneId) handler(event.payload);
  });
}
