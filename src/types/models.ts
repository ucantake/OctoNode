// Mirrors `src-tauri/src/models.rs` (serde camelCase). Keep in sync.

export type Uuid = string;

// ---------------------------------------------------------------------------
// Accounts & workspaces
// ---------------------------------------------------------------------------

export type GitHostType = "gitHub" | "gitLabCloud" | "gitLabSelfHosted" | "local";

export interface GitIdentity {
  name: string;
  email: string;
}

export interface SshSettings {
  privateKeyPath: string | null;
  useAgent: boolean;
}

export interface Account {
  id: Uuid;
  label: string;
  host: GitHostType;
  apiBaseUrl: string | null;
  username: string;
  avatarUrl: string | null;
  identity: GitIdentity;
  ssh: SshSettings;
  color: string | null;
}

export type AccountInput = Omit<Account, "id">;

export type AccountStatus = "ready" | "missingToken" | "locked";

export interface AccountView extends Account {
  status: AccountStatus;
}

export interface RepositoryRef {
  id: Uuid;
  name: string;
  path: string;
  organization: string | null;
  remoteUrl: string | null;
}

export interface Workspace {
  id: Uuid;
  name: string;
  accountId: Uuid | null;
  repositories: RepositoryRef[];
}

export type SecretBackendKind = "osKeyring" | "encryptedFile";

export interface VaultStatus {
  backend: SecretBackendKind;
  locked: boolean;
  initialized: boolean;
  fallbackReason: string | null;
}

export interface PlatformInfo {
  os: "linux" | "windows" | "macos" | string;
  pathSeparator: string;
  primaryModifier: "Meta" | "Control";
}

export interface BootstrapState {
  accounts: AccountView[];
  workspaces: Workspace[];
  activeWorkspaceId: Uuid | null;
  vault: VaultStatus;
  platform: PlatformInfo;
  configDir: string;
}

// ---------------------------------------------------------------------------
// Commit graph
// ---------------------------------------------------------------------------

export type RefKind = "localBranch" | "remoteBranch" | "tag" | "head";

export interface RefBadge {
  name: string;
  kind: RefKind;
  isHead: boolean;
}

/** Segment entering a row from the previous row: [fromLane, toLane, color]. */
export type GraphEdge = [number, number, number];

export interface CommitNode {
  id: string;
  shortId: string;
  summary: string;
  authorName: string;
  authorEmail: string;
  /** Unix seconds. */
  time: number;
  parents: string[];
  lane: number;
  color: number;
  edges: GraphEdge[];
  refs: RefBadge[];
}

export interface GraphPage {
  total: number;
  offset: number;
  maxLanes: number;
  truncated: boolean;
  rows: CommitNode[];
}

// ---------------------------------------------------------------------------
// Diffs
// ---------------------------------------------------------------------------

export type DiffTarget =
  | { type: "workingTree" }
  | { type: "index" }
  | { type: "commit"; id: string };

export type DiffLineKind = "context" | "addition" | "deletion";

export interface DiffLine {
  kind: DiffLineKind;
  oldLineno: number | null;
  newLineno: number | null;
  content: string;
  noNewline: boolean;
}

export interface Hunk {
  header: string;
  oldStart: number;
  oldLines: number;
  newStart: number;
  newLines: number;
  lines: DiffLine[];
}

export type FileStatus =
  | "added"
  | "deleted"
  | "modified"
  | "renamed"
  | "copied"
  | "typeChange"
  | "untracked"
  | "conflicted"
  | "unmodified";

export interface FileDiff {
  oldPath: string | null;
  newPath: string | null;
  status: FileStatus;
  isBinary: boolean;
  truncated: boolean;
  additions: number;
  deletions: number;
  hunks: Hunk[];
}

export interface DiffResult {
  target: DiffTarget;
  contextLines: number;
  files: FileDiff[];
}

export interface HunkSelection {
  hunkIndex: number;
  header: string;
  /** Indices into `Hunk.lines`; `null` = whole hunk. */
  lines: number[] | null;
}

export type StageAction = "stage" | "unstage";

export interface StageRequest {
  repoId: Uuid;
  path: string;
  action: StageAction;
  contextLines: number;
  /** Empty = whole file. */
  hunks: HunkSelection[];
}

export interface FetchOutcome {
  remote: string;
  transport: "libgit2" | "gitCli";
  receivedObjects: number;
}

// ---------------------------------------------------------------------------
// Settings & cloning
// ---------------------------------------------------------------------------

export interface SettingsView {
  /** Effective parent folder for new clones. */
  cloneDirectory: string;
  cloneDirectoryIsDefault: boolean;
  vault: VaultStatus;
  configDir: string;
  dataDir: string;
  appVersion: string;
}

export interface SettingsInput {
  /** `null` / empty = default (`~/Projects`). */
  cloneDirectory: string | null;
}

export interface RemoteRepo {
  fullName: string;
  name: string;
  namespace: string;
  description: string | null;
  httpsUrl: string;
  sshUrl: string;
  webUrl: string;
  defaultBranch: string | null;
  private: boolean;
  updatedAt: string | null;
}

export interface CloneRequest {
  cloneId: Uuid;
  workspaceId: Uuid;
  url: string;
  parentDirectory: string | null;
  folderName: string | null;
  bindIdentity: boolean;
}

export type ClonePhase = "receiving" | "resolving" | "checkout" | "fallback" | "done";

export interface CloneProgress {
  cloneId: Uuid;
  phase: ClonePhase;
  receivedObjects: number;
  totalObjects: number;
  indexedDeltas: number;
  totalDeltas: number;
  receivedBytes: number;
  checkoutDone: number;
  checkoutTotal: number;
}

export interface CloneResult {
  repository: RepositoryRef;
  transport: "libgit2" | "gitCli";
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

export type AppErrorKind =
  | "git"
  | "io"
  | "serialization"
  | "secretStore"
  | "vaultLocked"
  | "invalidPassword"
  | "notFound"
  | "invalidInput"
  | "stale"
  | "unsupported"
  | "remote"
  | "cancelled"
  | "process"
  | "internal";

export interface AppErrorPayload {
  kind: AppErrorKind;
  message: string;
}
