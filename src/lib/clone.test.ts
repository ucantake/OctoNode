import { describe, expect, it } from "vitest";
import { accountHost, cloneProgressFraction, folderNameFromUrl, formatBytes, newCloneId, urlHost } from "./clone";
import type { CloneProgress } from "../types/models";

const base: CloneProgress = {
  cloneId: "x",
  phase: "receiving",
  receivedObjects: 0,
  totalObjects: 0,
  indexedDeltas: 0,
  totalDeltas: 0,
  receivedBytes: 0,
  checkoutDone: 0,
  checkoutTotal: 0,
};

describe("folderNameFromUrl", () => {
  it("matches the Rust implementation", () => {
    expect(folderNameFromUrl("https://github.com/acme/api.git")).toBe("api");
    expect(folderNameFromUrl("git@git.corp:group/sub/tool")).toBe("tool");
    expect(folderNameFromUrl("ssh://h/x/repo.git/")).toBe("repo");
    expect(folderNameFromUrl("https://github.com/")).toBeNull();
    expect(folderNameFromUrl("")).toBeNull();
  });
});

describe("cloneProgressFraction", () => {
  it("is monotonic across phases", () => {
    const r = cloneProgressFraction({ ...base, receivedObjects: 50, totalObjects: 100 });
    const d = cloneProgressFraction({ ...base, phase: "resolving", indexedDeltas: 5, totalDeltas: 10 });
    const c = cloneProgressFraction({ ...base, phase: "checkout", checkoutDone: 1, checkoutTotal: 2 });
    expect(r).toBeCloseTo(0.35);
    expect(d).toBeCloseTo(0.8);
    expect(c).toBeCloseTo(0.95);
    expect(cloneProgressFraction({ ...base, phase: "done" })).toBe(1);
  });

  it("is indeterminate without totals", () => {
    expect(cloneProgressFraction(base)).toBeNull();
    expect(cloneProgressFraction({ ...base, phase: "fallback" })).toBeNull();
  });
});

describe("misc", () => {
  it("formats bytes", () => {
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(1536)).toBe("1.5 KiB");
    expect(formatBytes(5 * 1024 * 1024)).toBe("5.0 MiB");
  });

  it("generates v4 UUIDs", () => {
    expect(newCloneId()).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
  });
});

describe("hosts", () => {
  it("extracts URL hosts", () => {
    expect(urlHost("https://GitHub.com/a/b.git")).toBe("github.com");
    expect(urlHost("ssh://git@git.corp.io:2222/g/r.git")).toBe("git.corp.io");
    expect(urlHost("git@gitlab.com:g/r.git")).toBe("gitlab.com");
    expect(urlHost("nonsense")).toBeNull();
  });

  it("derives account hosts", () => {
    expect(accountHost({ host: "gitHub", apiBaseUrl: null })).toBe("github.com");
    expect(accountHost({ host: "gitLabSelfHosted", apiBaseUrl: "https://Git.Corp.io/api/v4" })).toBe("git.corp.io");
    expect(accountHost({ host: "local", apiBaseUrl: null })).toBeNull();
  });
});
