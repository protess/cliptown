#!/usr/bin/env node
/**
 * M10.1 regression gate. Reads `target/criterion/<bench>/new/estimates.json`
 * for each metric declared in `bench/baselines.json`, applies the configured
 * `extract` recipe to turn the criterion median (always nanoseconds) into the
 * baseline's unit, then compares the result against the saved baseline. Exits
 * 1 with a JSON summary on stdout if any metric regressed by more than the
 * file-level `tolerance_pct`.
 *
 * Frontend metrics (`baseline: null` with a `ceiling`) are skipped here — the
 * Playwright FCP bench asserts those directly at the test-runner level.
 */

import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(HERE, "..");

/** Convert a criterion median (nanoseconds) into the baseline's unit. */
function applyExtract(extract, medianNs) {
  switch (extract) {
    case "median_ns_to_us":
      return medianNs / 1_000;
    case "100_div_median_us":
      // console_dispatch_throughput bench fires 100 msgs per iter;
      // throughput = 100 msgs / (median_ns → sec).
      return 100 / (medianNs / 1_000 / 1_000_000);
    default:
      throw new Error(`unknown extract recipe: ${extract}`);
  }
}

async function loadCriterionMedian(benchName) {
  const path = resolve(
    REPO_ROOT,
    "target",
    "criterion",
    benchName,
    "new",
    "estimates.json",
  );
  const buf = await readFile(path, "utf-8");
  const j = JSON.parse(buf);
  if (!j.median || typeof j.median.point_estimate !== "number") {
    throw new Error(`malformed criterion estimates at ${path}`);
  }
  return j.median.point_estimate;
}

/**
 * Regression direction depends on the metric. tick_latency is "lower is
 * better" — current > baseline by >tolerance is a regression. throughput is
 * "higher is better" — current < baseline by >tolerance is a regression. We
 * encode this by always normalizing to "ratio (current - baseline) / baseline"
 * and flipping the sign on throughput metrics.
 */
function isRegression(metricName, currentValue, baseline, tolerancePct) {
  const ratio = (currentValue - baseline) / baseline;
  const higherIsBetter = metricName.endsWith("_msgs_per_sec");
  const directional = higherIsBetter ? -ratio : ratio;
  return directional > tolerancePct / 100;
}

async function main() {
  const baselinesPath = resolve(REPO_ROOT, "bench", "baselines.json");
  const baselinesRaw = await readFile(baselinesPath, "utf-8");
  const baselines = JSON.parse(baselinesRaw);
  const tolerance = baselines.tolerance_pct ?? 20;
  const results = [];
  let anyFailed = false;

  for (const [name, m] of Object.entries(baselines.metrics)) {
    if (m.baseline === null || m.baseline === undefined) {
      results.push({ name, status: "skipped", reason: "no baseline (frontend gate)" });
      continue;
    }
    if (!m.criterion_bench) {
      results.push({ name, status: "skipped", reason: "no criterion_bench mapping" });
      continue;
    }
    let medianNs;
    try {
      medianNs = await loadCriterionMedian(m.criterion_bench);
    } catch (e) {
      results.push({ name, status: "error", error: String(e) });
      anyFailed = true;
      continue;
    }
    const current = applyExtract(m.extract, medianNs);
    const regressed = isRegression(name, current, m.baseline, tolerance);
    results.push({
      name,
      status: regressed ? "regressed" : "ok",
      baseline: m.baseline,
      current: Number(current.toFixed(4)),
      unit: m.unit,
      delta_pct: Number(((current - m.baseline) / m.baseline * 100).toFixed(2)),
    });
    if (regressed) anyFailed = true;
  }

  process.stdout.write(
    JSON.stringify({ ok: !anyFailed, tolerance_pct: tolerance, results }, null, 2) + "\n",
  );
  process.exit(anyFailed ? 1 : 0);
}

main().catch((e) => {
  process.stdout.write(
    JSON.stringify({ ok: false, error: e instanceof Error ? e.message : String(e) }) + "\n",
  );
  process.exit(1);
});
