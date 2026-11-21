export interface User {
  id: string;
  email: string;
  handle: string;
  display_name: string;
  is_admin: boolean;
  bandwidth_quota_mbps: number;
}

export interface EndpointCandidate {
  kind: string;
  addr: string;
}

export interface Device {
  id: string;
  name: string;
  role: string;
  platform: string;
  ed25519_pubkey?: string;
  x25519_pubkey: string;
  endpoints_json?: EndpointCandidate[];
  herdr_version?: string | null;
  is_online: boolean;
}

export interface RelayNodeInfo {
  id: string;
  region: string;
  mode: string;
  endpoint: string;
  priority: number;
}

export interface HerdrTerminalStatus {
  state: string;
  kind?: string | null;
  app?: string | null;
  title?: string | null;
  msg?: string | null;
}

export interface HerdrTerminal {
  address: string;
  name?: string | null;
  argv: string[];
  cwd: string;
  alive: boolean;
  status?: HerdrTerminalStatus | null;
}

export interface BranchWorktreeItem {
  branch: string;
  worktree_path: string;
  diff_added: number;
  diff_deleted: number;
  active_terminals: HerdrTerminal[];
  status_summary: string;
}

export interface RepoWorkspaceSummary {
  repo_name: string;
  root_path: string;
  machine_name: string;
  branches: BranchWorktreeItem[];
}

export interface VoiceModelConfig {
  provider: string;
  base_url: string;
  api_key: string;
  model: string;
}

export interface AuthSessionInfo {
  id: string;
  client_label: string;
  scope: string;
  device_id?: string | null;
  created_at: string;
  last_used_at: string;
  expires_at: string;
  is_current: boolean;
}

export interface InviteCodeInfo {
  code: string;
  note?: string | null;
  max_uses: number;
  used_count: number;
  expires_at?: string | null;
  created_at: string;
}

export interface AdminUserSummary {
  id: string;
  handle: string;
  email: string;
  display_name: string;
  is_admin: boolean;
  online_devices: number;
  total_devices: number;
  last_login_at?: string | null;
  created_at: string;
}

export interface PromptSnippet {
  id: string;
  title: string;
  prompt: string;
}

export interface VapidPublicKeyInfo {
  enabled: boolean;
  public_key: string;
}

export interface SiteInfo {
  version: string;
  open_registration: boolean;
  has_any_users: boolean;
  public_base_url: string;
  install_command: string;
  downloads: {
    daemon_linux_x64: string;
    daemon_linux_arm64: string;
    daemon_macos_arm64: string;
    server_linux_x64: string;
    server_linux_arm64: string;
    server_macos_arm64: string;
    desktop_macos_dmg: string;
    desktop_macos_tar: string;
    deploy_bundle: string;
    ios_beta_url?: string | null;
    releases_page: string;
  };
}

export interface DeviceAuthPublicInfo {
  user_code: string;
  device_name: string;
  role: string;
  platform: string;
  status: string;
  expires_at: string;
}

export type ThemePreference = "light" | "dark" | "system";
export type Locale = "en" | "zh";
export type AppRoute = "home" | "login" | "register" | "activate" | "reset-password" | "console";
export type TransportMode = "lan_ws" | "relay_ws";

export function readSessionOrLegacyToken(): string | null {
  try {
    if (typeof sessionStorage !== "undefined") {
      const s = sessionStorage.getItem("coda_token");
      if (s) return s;
      if (typeof localStorage !== "undefined") {
        const legacy = localStorage.getItem("coda_token");
        if (legacy) {
          sessionStorage.setItem("coda_token", legacy);
          localStorage.removeItem("coda_token");
          return legacy;
        }
      }
      return null;
    }
    if (typeof localStorage !== "undefined") return localStorage.getItem("coda_token");
  } catch {
    // ignore storage access errors
  }
  return null;
}

export function saveSessionToken(token: string): void {
  try {
    if (typeof sessionStorage !== "undefined") sessionStorage.setItem("coda_token", token);
    if (typeof localStorage !== "undefined") localStorage.removeItem("coda_token");
  } catch {
    // ignore storage errors
  }
}

export function clearSessionToken(): void {
  try {
    if (typeof sessionStorage !== "undefined") sessionStorage.removeItem("coda_token");
    if (typeof localStorage !== "undefined") localStorage.removeItem("coda_token");
  } catch {
    // ignore storage errors
  }
}

export function readLocalPref(key: string, fallback: string): string {
  try {
    if (typeof localStorage !== "undefined") return localStorage.getItem(key) || fallback;
  } catch {
    // ignore
  }
  return fallback;
}

export function writeLocalPref(key: string, val: string): void {
  try {
    if (typeof localStorage !== "undefined") localStorage.setItem(key, val);
  } catch {
    // ignore
  }
}
