import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";

/** Opening an `.rrd` that carries per-model torque shows the torque charts.
 *
 * `orts run --format rrd` records a torque per model (#466) and the live wire
 * carries the same values (#470), so a recording opened in the viewer has to
 * reach the same charts. The recording is produced here rather than committed:
 * it is a binary, and the CLI that writes it is in this repository.
 */

const __dirname = path.dirname(fileURLToPath(import.meta.url));

const CONFIG = {
  body: "earth",
  epoch: "2026-03-03T00:00:00Z",
  dt: 1.0,
  duration: 600,
  output_interval: 10,
  satellites: [
    {
      id: "sat-a",
      name: "sat-a",
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

let recording: string;
let configPath: string;

test.beforeAll(() => {
  const binary = process.env.ORTS_BINARY ?? path.resolve(__dirname, "../../target/debug/orts");
  configPath = path.join(os.tmpdir(), `orts-rrd-torque-${Date.now()}.json`);
  recording = path.join(os.tmpdir(), `orts-rrd-torque-${Date.now()}.rrd`);
  fs.writeFileSync(configPath, JSON.stringify(CONFIG));
  execFileSync(binary, ["run", "--config", configPath, "--format", "rrd", "--output", recording], {
    env: { ...process.env, ORTS_DISABLE_TEXTURE_DOWNLOAD: "1" },
  });
});

test.afterAll(() => {
  for (const file of [recording, configPath]) {
    try {
      fs.unlinkSync(file);
    } catch {
      // ignore
    }
  }
});

test("an .rrd carrying per-model torque shows its torque charts", async ({ page }) => {
  await page.goto("/?noAutoConnect=1");
  await page.locator('input[type="file"]').setInputFiles(recording);

  const titled = (title: string) => page.locator(".u-title", { hasText: new RegExp(`^${title}$`) });

  // The charts are keyed on the models the recording reports, so this covers
  // the decode, the point rebuild, and the metadata together.
  await expect(titled("Gravity-gradient Torque")).toBeVisible({ timeout: 60000 });
  await expect(titled("SRP Torque")).toBeVisible();
  await expect(titled("Aerodynamic Torque")).toBeVisible();

  // And the values, which a title alone does not show: the run tilts
  // `diag(10, 40, 45)` by 45 degrees about y at 400 km, where the
  // gravity-gradient torque is about 6.7e-5 N·m.
  const measured = await page.waitForFunction(
    () => {
      const data = (window as unknown as Record<string, unknown>).__debug_chart_data as Record<
        string,
        Float64Array
      > | null;
      const gg = data?.torque_gravity_gradient_y;
      if (!gg) return null;
      const finite = Array.from(gg).filter((v) => Number.isFinite(v));
      if (finite.length === 0) return null;
      return { max: Math.max(...finite.map(Math.abs)), samples: finite.length };
    },
    { timeout: 60000 },
  );
  const values = await measured.jsonValue();
  console.log("torque from the recording:", JSON.stringify(values));
  expect(values.max).toBeGreaterThan(1e-5);
});
