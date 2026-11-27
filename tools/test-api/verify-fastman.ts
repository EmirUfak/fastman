import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const API_ORIGIN = process.env.FASTMAN_TEST_ORIGIN ?? "http://127.0.0.1:4010";
const REPO_ROOT = resolve(import.meta.dir, "../..");
const CLI_BIN = join(REPO_ROOT, "target/debug/fastman-cli");
const COLLECTION = join(import.meta.dir, "fastman-collection.json");

function runCli(home: string, args: string[]) {
  const proc = Bun.spawnSync([CLI_BIN, ...args], {
    env: { ...process.env, FASTMAN_HOME: home },
    stdout: "pipe",
    stderr: "pipe",
  });
  return {
    code: proc.exitCode,
    stdout: proc.stdout.toString().trim(),
    stderr: proc.stderr.toString().trim(),
  };
}

function findRequestId(project: any, name: string): string {
  const walk = (col: any): string | null => {
    for (const r of col.requests ?? []) {
      if (r.name === name) return r.id;
    }
    for (const f of col.folders ?? []) {
      const found = walk(f);
      if (found) return found;
    }
    return null;
  };
  for (const col of project.collections ?? []) {
    const found = walk(col);
    if (found) return found;
  }
  throw new Error(`Request not found: ${name}`);
}

const seedFlagIdx = process.argv.indexOf("--seed");
const seedDir = seedFlagIdx >= 0 ? process.argv[seedFlagIdx + 1] : null;
const home = seedDir
  ? (rmSync(seedDir, { recursive: true, force: true }), resolve(seedDir))
  : mkdtempSync(join(tmpdir(), "fastman-live-test-"));
try {
  if (!seedDir) {
    // Reset server state
    const reset = await fetch(`${API_ORIGIN}/reset`, { method: "POST" });
    if (reset.status !== 204) throw new Error(`Reset failed: ${reset.status}`);
  }

  // 1. Import collection via fastman-cli
  const imp = runCli(home, ["import-collection", "--allow-plaintext-secrets", COLLECTION]);
  if (imp.code !== 0) {
    throw new Error(`import-collection failed (${imp.code}): ${imp.stderr}`);
  }
  console.log("1. Import OK:", imp.stdout);

  // Locate imported project JSON
  const list = runCli(home, ["list"]);
  const projectId = list.stdout.split("\t")[0]?.trim();
  if (!projectId) throw new Error(`No project listed: ${list.stdout}`);
  const projectPath = join(home, projectId, "project.json");
  const project = JSON.parse(readFileSync(projectPath, "utf8"));

  const loginId = findRequestId(project, "Login (sets cookie + token)");
  const meId = findRequestId(project, "Who am I (cookie or bearer)");
  const cookieOnlyId = findRequestId(project, "Cookie-only check (tests isolation)");
  const logoutId = findRequestId(project, "Logout");
  const createId = findRequestId(project, "Create item");
  const readId = findRequestId(project, "Read item");
  const updateId = findRequestId(project, "Update item");
  const deleteId = findRequestId(project, "Delete item (cleanup)");
  const verifyDeletedId = findRequestId(project, "Verify item deleted (404)");

  // Chain 1: Full Auth + Cookie + Isolation + CRUD Lifecycle
  project.chains = [
    {
      id: "crud-lifecycle",
      name: "Auth + Cookies + CRUD Lifecycle",
      steps: [
        {
          id: "s-login",
          request_id: loginId,
          enabled: true,
          assertions: [{ kind: "status", expected: 200 }],
          captures: [
            {
              name: "access_token",
              source: { kind: "json_field", pointer: "/auth/access_token" },
              required: true,
              sensitive: true,
            },
          ],
        },
        {
          id: "s-cookie-shared",
          request_id: cookieOnlyId,
          enabled: true,
          assertions: [
            { kind: "status", expected: 200 },
            { kind: "json_field", pointer: "/has_cookie", expected: true },
          ],
        },
        {
          id: "s-cookie-isolated",
          request_id: cookieOnlyId,
          enabled: true,
          isolate_cookies: true,
          assertions: [
            { kind: "status", expected: 401 },
            { kind: "json_field", pointer: "/has_cookie", expected: false },
          ],
        },
        {
          id: "s-me-both",
          request_id: meId,
          enabled: true,
          inputs: [
            {
              source_step_id: "s-login",
              capture_name: "access_token",
              target: { kind: "bearer_token" },
            },
          ],
          assertions: [
            { kind: "status", expected: 200 },
            { kind: "json_field", pointer: "/auth_via", expected: "cookie+bearer" },
          ],
        },
        {
          id: "s-create",
          request_id: createId,
          enabled: true,
          inputs: [
            {
              source_step_id: "s-login",
              capture_name: "access_token",
              target: { kind: "bearer_token" },
            },
          ],
          assertions: [
            { kind: "status", expected: 201 },
            { kind: "json_field", pointer: "/version", expected: 1 },
          ],
          captures: [
            {
              name: "item_id",
              source: { kind: "json_field", pointer: "/id" },
              required: true,
              sensitive: false,
            },
          ],
        },
        {
          id: "s-read",
          request_id: readId,
          enabled: true,
          inputs: [
            {
              source_step_id: "s-create",
              capture_name: "item_id",
              target: { kind: "variable", name: "item_id" },
            },
          ],
          assertions: [
            { kind: "status", expected: 200 },
            { kind: "json_field", pointer: "/version", expected: 1 },
          ],
        },
        {
          id: "s-update",
          request_id: updateId,
          enabled: true,
          inputs: [
            {
              source_step_id: "s-create",
              capture_name: "item_id",
              target: { kind: "variable", name: "item_id" },
            },
          ],
          assertions: [
            { kind: "status", expected: 200 },
            { kind: "json_field", pointer: "/version", expected: 2 },
            { kind: "json_field", pointer: "/status", expected: "archived" },
          ],
        },
        {
          id: "s-delete",
          request_id: deleteId,
          enabled: true,
          always_run: true,
          inputs: [
            {
              source_step_id: "s-create",
              capture_name: "item_id",
              target: { kind: "variable", name: "item_id" },
            },
          ],
          assertions: [{ kind: "status", expected: 204 }],
        },
        {
          id: "s-verify-404",
          request_id: verifyDeletedId,
          enabled: true,
          inputs: [
            {
              source_step_id: "s-create",
              capture_name: "item_id",
              target: { kind: "variable", name: "item_id" },
            },
          ],
          assertions: [{ kind: "status", expected: 404 }],
        },
        {
          id: "s-logout",
          request_id: logoutId,
          enabled: true,
          assertions: [{ kind: "status", expected: 204 }],
        },
      ],
    },
    {
      id: "cleanup-on-failure",
      name: "Cleanup runs after failed assertion",
      steps: [
        {
          id: "f-login",
          request_id: loginId,
          enabled: true,
          assertions: [{ kind: "status", expected: 200 }],
        },
        {
          id: "f-create",
          request_id: createId,
          enabled: true,
          assertions: [{ kind: "status", expected: 201 }],
          captures: [
            {
              name: "item_id",
              source: { kind: "json_field", pointer: "/id" },
              required: true,
              sensitive: false,
            },
          ],
        },
        {
          id: "f-read-fail",
          request_id: readId,
          enabled: true,
          inputs: [
            {
              source_step_id: "f-create",
              capture_name: "item_id",
              target: { kind: "variable", name: "item_id" },
            },
          ],
          assertions: [{ kind: "json_field", pointer: "/version", expected: 999 }],
        },
        {
          id: "f-update-skip",
          request_id: updateId,
          enabled: true,
          inputs: [
            {
              source_step_id: "f-create",
              capture_name: "item_id",
              target: { kind: "variable", name: "item_id" },
            },
          ],
        },
        {
          id: "f-delete-cleanup",
          request_id: deleteId,
          enabled: true,
          always_run: true,
          inputs: [
            {
              source_step_id: "f-create",
              capture_name: "item_id",
              target: { kind: "variable", name: "item_id" },
            },
          ],
          assertions: [{ kind: "status", expected: 204 }],
        },
      ],
    },
  ];
  writeFileSync(projectPath, JSON.stringify(project, null, 2));
  if (seedDir) {
    console.log(`Seeded ${home} with project "${project.name}" and 2 chains.`);
    process.exit(0);
  }

  // 1b. Verify chain list CLI
  const listed = runCli(home, ["chain", "list", "--project", projectId, "--json"]);
  if (listed.code !== 0) {
    throw new Error(`chain list failed (${listed.code}): ${listed.stderr}`);
  }
  const listedChains = JSON.parse(listed.stdout).chains;
  if (!Array.isArray(listedChains) || listedChains.length !== 2) {
    throw new Error(`Expected 2 listed chains, got: ${listed.stdout}`);
  }
  console.log("1b. chain list OK:", listedChains.map((c: any) => `${c.id}(${c.steps})`).join(", "));

  // 2. Run Chain 1
  const run1 = runCli(home, [
    "chain",
    "run",
    "--project",
    projectId,
    "--chain",
    "crud-lifecycle",
    "--origin",
    API_ORIGIN,
    "--json",
  ]);
  if (run1.code !== 0) {
    throw new Error(`Chain 1 failed (exit ${run1.code}):\n${run1.stdout}\n${run1.stderr}`);
  }
  const report1 = JSON.parse(run1.stdout);
  if (report1.steps.length !== 10 || !report1.steps.every((s: any) => s.state === "passed")) {
    throw new Error(`Chain 1 unexpected report: ${run1.stdout}`);
  }
  if (run1.stdout.includes("fm_")) {
    throw new Error("Security violation: access_token leaked in CLI JSON output!");
  }
  console.log("2. Chain 1 (10 steps: auth+cookies+isolation+CRUD+logout) PASSED, secrets redacted.");

  // 3. Run Chain 2 (failure + always_run cleanup)
  const run2 = runCli(home, [
    "chain",
    "run",
    "--project",
    projectId,
    "--chain",
    "cleanup-on-failure",
    "--origin",
    API_ORIGIN,
    "--json",
  ]);
  const report2 = JSON.parse(run2.stdout);
  const states2 = report2.steps.map((s: any) => s.state);
  console.log("3. Chain 2 exit:", run2.code, "states:", states2);
  if (run2.code !== 4) {
    throw new Error(`Expected exit code 4 (assertion failure), got ${run2.code}`);
  }
  const expected2 = ["passed", "passed", "failed", "skipped", "passed"];
  if (JSON.stringify(states2) !== JSON.stringify(expected2)) {
    throw new Error(`Expected ${JSON.stringify(expected2)}, got ${JSON.stringify(states2)}`);
  }

  // Verify server has 0 leftover items after cleanup!
  const health = await (await fetch(`${API_ORIGIN}/health`)).json();
  if (health.items !== 0) {
    throw new Error(`Cleanup failed! Server still has ${health.items} items.`);
  }
  console.log("4. Server health after cleanup:", JSON.stringify(health));
} finally {
  if (!seedDir) {
    rmSync(home, { recursive: true, force: true });
  }
}
