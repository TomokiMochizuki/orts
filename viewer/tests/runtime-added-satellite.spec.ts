import { type ChildProcess, spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";
import { expect, type Page, test } from "@playwright/test";

/** A satellite added to a running simulation brings its own charts.
 *
 * The chart-visibility list is the union of the models each satellite
 * reports, so this covers the whole contract: the server naming the models it
 * built for the new satellite, the announcement reaching the viewer, and the
 * merge into the Info snapshot the charts are keyed on. The unit tests cover
 * the merge rule; only the running app covers the wiring that hands the
 * dispatcher the snapshot to merge into.
 */

const __dirname = path.dirname(fileURLToPath(import.meta.url));

/** One orbit-only satellite with no ballistic coefficient: no drag model.
 *
 * Orbit-only is the mode `add_satellite` accepts — a plain attitude group
 * refuses one — so the run starts here and the added satellite brings the
 * model this one lacks.
 */
const NO_DRAG = {
  body: "earth",
  epoch: "2026-03-03T00:00:00Z",
  dt: 1.0,
  output_interval: 5,
  satellites: [
    {
      id: "sat-a",
      name: "sat-a",
      orbit: { type: "circular", altitude: 500 },
    },
  ],
};

/** The satellite added at runtime, whose ballistic coefficient brings drag. */
const ADDED = {
  id: "sat-b",
  name: "sat-b",
  orbit: { type: "circular", altitude: 400 },
  ballistic_coeff: 50.0,
};

let server: ChildProcess | undefined;
let wsUrl: string;
let configPath: string;

test.beforeAll(async () => {
  configPath = path.join(os.tmpdir(), `orts-added-sat-e2e-${Date.now()}.json`);
  fs.writeFileSync(configPath, JSON.stringify(NO_DRAG));

  const binary = process.env.ORTS_BINARY ?? path.resolve(__dirname, "../../target/debug/orts");
  const child = spawn(binary, ["serve", "--port", "0", "--config", configPath], {
    env: { ...process.env, ORTS_DISABLE_TEXTURE_DOWNLOAD: "1" },
  });
  server = child;

  const port = await new Promise<number>((resolve, reject) => {
    const rl = createInterface({ input: child.stderr ?? process.stdin });
    const timeout = setTimeout(() => {
      rl.close();
      reject(new Error("Timed out waiting for the orts server to start"));
    }, 30000);
    rl.on("line", (line) => {
      const match = line.match(/ws:\/\/localhost:(\d+)/);
      if (match) {
        clearTimeout(timeout);
        resolve(Number.parseInt(match[1], 10));
      }
    });
    child.on("error", (err) => {
      clearTimeout(timeout);
      reject(err);
    });
    child.on("exit", (code) => {
      clearTimeout(timeout);
      reject(new Error(`orts exited with code ${code} before listening`));
    });
  });

  wsUrl = `ws://localhost:${port}/ws`;
});

test.afterAll(async () => {
  if (server && !server.killed) server.kill("SIGTERM");
  if (configPath) {
    try {
      fs.unlinkSync(configPath);
    } catch {
      // ignore
    }
  }
});

async function connectTo(page: Page, url: string) {
  await page.goto("/");
  const disconnectBtn = page.locator('[data-testid="ws-disconnect-btn"]');
  try {
    await disconnectBtn.waitFor({ state: "visible", timeout: 3000 });
    await disconnectBtn.click();
  } catch {
    // Not connected to the default server; continue.
  }
  await page.locator('[data-testid="ws-url-input"]').fill(url);
  await page.locator('[data-testid="ws-connect-btn"]').click();
}

function chartTitled(page: Page, title: string) {
  return page.locator(".u-title", { hasText: new RegExp(`^${title}$`) });
}

test("a satellite added at runtime brings the charts for its own models", async ({ page }) => {
  await connectTo(page, wsUrl);

  // Wait for a chart the run does have, so the absence below is measured
  // against a loaded panel. The initial satellite has no drag model.
  await expect(chartTitled(page, "Sun 3rd-body")).toBeVisible({ timeout: 40000 });
  await expect(chartTitled(page, "Drag")).toHaveCount(0);

  // Add one with a ballistic coefficient, through a second client. The server
  // broadcasts the announcement to every connection, the app's included.
  const sent = await page.evaluate(
    ([url, satellite]) =>
      new Promise<string>((resolve, reject) => {
        const ws = new WebSocket(url as string);
        ws.addEventListener("open", () => {
          // `add_satellite` flattens the satellite config into the envelope.
          ws.send(JSON.stringify({ type: "add_satellite", ...(satellite as object) }));
        });
        ws.addEventListener("message", (e) => {
          const msg = JSON.parse(e.data as string);
          if (msg.type === "satellite_added") {
            ws.close();
            resolve(JSON.stringify(msg.satellite.perturbations));
          }
          if (msg.type === "error") {
            ws.close();
            reject(new Error(msg.message));
          }
        });
        setTimeout(() => {
          ws.close();
          reject(new Error("no satellite_added within 20s"));
        }, 20000);
      }),
    [wsUrl, ADDED] as const,
  );
  console.log("added satellite reports:", sent);
  // The server names the models it built for the new satellite; an empty list
  // here is the regression this whole path exists to prevent.
  expect(JSON.parse(sent)).toContain("drag");

  // And that model reaches the charts.
  await expect(chartTitled(page, "Drag")).toBeVisible({ timeout: 40000 });
  // What the run already had stays: the merge adds to the snapshot rather
  // than replacing it.
  await expect(chartTitled(page, "Sun 3rd-body")).toBeVisible();
});
