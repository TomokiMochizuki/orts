import { type ChildProcess, spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";
import { expect, type Page, test } from "@playwright/test";

/** The torque charts, end to end from `orts serve` to the rendered chart.
 *
 * The wire, the DuckDB columns and the chart-visibility rule have unit tests;
 * what none of them can show is whether a chart actually appears for a model
 * the server reports, with that model's three axes in it. That path runs
 * through the WebSocket, the DuckDB store and the chart component.
 */

const __dirname = path.dirname(fileURLToPath(import.meta.url));

/** An off-centre panel on a tilted spacecraft, so every model has a torque.
 *
 * The epoch is what lets the SRP model see the Sun — without one `PanelSrp`
 * reports nothing. `cp_offset` puts the centre of pressure off the centre of
 * mass, which is what turns a force into a torque, and the initial quaternion
 * tilts the body 45 degrees about y, where this inertia's gravity-gradient
 * torque is largest.
 */
const PANEL_CONFIG = {
  body: "earth",
  epoch: "2026-03-03T00:00:00Z",
  dt: 1.0,
  output_interval: 10,
  satellites: [
    {
      id: "torque-test",
      name: "Torque Test",
      orbit: { type: "circular", altitude: 400 },
      attitude: {
        mass: 500,
        inertia_diag: [10, 40, 45],
        initial_quaternion: [0.9238795325112867, 0, 0.3826834323650898, 0],
      },
      panels: [
        {
          area: 2.0,
          normal: [1.0, 0.0, 0.0],
          cd: 2.2,
          specular: 0.2,
          diffuse: 0.1,
          cp_offset: [0.0, 0.0, 1.5],
        },
      ],
    },
  ],
};

const ORBIT_ONLY_CONFIG = {
  body: "earth",
  dt: 1.0,
  output_interval: 10,
  satellites: [{ id: "orbit-only", orbit: { type: "circular", altitude: 500 } }],
};

interface Server {
  child: ChildProcess;
  wsUrl: string;
  configPath: string;
}

async function startServer(config: unknown, tag: string): Promise<Server> {
  const configPath = path.join(os.tmpdir(), `orts-torque-e2e-${tag}-${Date.now()}.json`);
  fs.writeFileSync(configPath, JSON.stringify(config));

  const binary = process.env.ORTS_BINARY ?? path.resolve(__dirname, "../../target/debug/orts");
  const child = spawn(binary, ["serve", "--port", "0", "--config", configPath], {
    env: { ...process.env, ORTS_DISABLE_TEXTURE_DOWNLOAD: "1" },
  });

  const port = await new Promise<number>((resolve, reject) => {
    const rl = createInterface({ input: child.stderr ?? process.stdin });
    const timeout = setTimeout(() => {
      rl.close();
      reject(new Error(`Timed out waiting for the ${tag} server to start`));
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

  return { child, wsUrl: `ws://localhost:${port}/ws`, configPath };
}

function stopServer(server: Server | undefined) {
  if (!server) return;
  if (!server.child.killed) server.child.kill("SIGTERM");
  try {
    fs.unlinkSync(server.configPath);
  } catch {
    // ignore
  }
}

/** Point the app at a server through its own connection form. */
async function connectTo(page: Page, wsUrl: string) {
  await page.goto("/");

  const disconnectBtn = page.locator('[data-testid="ws-disconnect-btn"]');
  try {
    await disconnectBtn.waitFor({ state: "visible", timeout: 3000 });
    await disconnectBtn.click();
  } catch {
    // Not connected to the default server; continue.
  }

  await page.locator('[data-testid="ws-url-input"]').fill(wsUrl);
  await page.locator('[data-testid="ws-connect-btn"]').click();
}

/** A chart by its title, as `uPlot` renders it. */
function chartTitled(page: Page, title: string) {
  return page.locator(".u-title", { hasText: new RegExp(`^${title}$`) });
}

let panelServer: Server | undefined;

test.beforeAll(async () => {
  panelServer = await startServer(PANEL_CONFIG, "panels");
});

test.afterAll(async () => {
  stopServer(panelServer);
  panelServer = undefined;
});

test("a chart appears for every model the server reports a torque for", async ({ page }) => {
  if (!panelServer) throw new Error("the panel server did not start");
  await connectTo(page, panelServer.wsUrl);

  // The charts are keyed on the models the info message names, so wait for a
  // title rather than for a fixed delay.
  await expect(chartTitled(page, "SRP Torque")).toBeVisible({ timeout: 40000 });
  await expect(chartTitled(page, "Gravity-gradient Torque")).toBeVisible();
  await expect(chartTitled(page, "Aerodynamic Torque")).toBeVisible();

  // The values have to arrive, not just the chart: `TimeSeriesChart` draws its
  // title and legend from the series configuration, so an all-gap chart looks
  // the same. These numbers come from the `orts serve` started above, through
  // the wire, `parseTorques`, the chart row and the live buffer.
  const columns = await page.waitForFunction(
    () => {
      const data = (window as unknown as Record<string, unknown>).__debug_chart_data as Record<
        string,
        Float64Array
      > | null;
      if (!data) return null;
      const gg = data.torque_gravity_gradient_y;
      const srp = data.torque_panel_srp_x;
      if (!gg || !srp) return null;
      const finite = (v: Float64Array) => Array.from(v).filter((x) => Number.isFinite(x));
      const ggFinite = finite(gg);
      const srpFinite = finite(srp);
      if (ggFinite.length === 0 || srpFinite.length === 0) return null;
      return {
        gravityGradientMax: Math.max(...ggFinite.map(Math.abs)),
        srpMax: Math.max(...srpFinite.map(Math.abs)),
        ggSamples: ggFinite.length,
      };
    },
    { timeout: 40000 },
  );
  const measured = await columns.jsonValue();
  console.log("torque columns:", JSON.stringify(measured));
  // A 45-degree tilt of `diag(10, 40, 45)` at 400 km produces about 6.7e-5 N·m
  // about y; the panel's SRP torque is orders smaller but not zero.
  expect(measured.gravityGradientMax).toBeGreaterThan(1e-5);
  expect(measured.srpMax).toBeGreaterThan(0);

  // Three series in one chart is what distinguishes a direction from a
  // magnitude, so the legend has to name all three axes.
  const srpChart = page.locator(".uplot", { has: chartTitled(page, "SRP Torque") });
  // The chart is drawn as soon as the run names the model, and its series
  // appear with the first sample, so wait for the count before reading labels.
  const legendEntries = srpChart.locator(".u-legend .u-series");
  await expect(legendEntries).toHaveCount(4, { timeout: 40000 });
  const legendLabels = await legendEntries.locator(".u-label").allInnerTexts();
  // The first entry labels the x axis (`uPlot` calls it "Value"); the rest are
  // the series, and there must be exactly the three body axes.
  expect(legendLabels.slice(1)).toEqual(["x", "y", "z"]);
});

test("no torque chart appears for a run that carries no torque model", async ({ page }) => {
  // Nothing in an orbit-only run reports a torque, so a chart that would be
  // empty must not be drawn at all.
  const server = await startServer(ORBIT_ONLY_CONFIG, "orbit");
  try {
    await connectTo(page, server.wsUrl);

    // Wait for a chart an orbit-only run does have, so the absence below is
    // measured against a loaded panel and not an empty one.
    await expect(chartTitled(page, "Altitude")).toBeVisible({ timeout: 40000 });
    await expect(chartTitled(page, "SRP Torque")).toHaveCount(0);
    await expect(chartTitled(page, "Gravity-gradient Torque")).toHaveCount(0);
    await expect(chartTitled(page, "Aerodynamic Torque")).toHaveCount(0);
  } finally {
    stopServer(server);
  }
});
