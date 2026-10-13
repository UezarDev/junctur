// `npm run tauri ...`: the Tauri CLI, but `dev` and `build` turn on the `cuda` feature when the CUDA 13
// toolkit is installed, so the app transcribes on the GPU (2l: the bare CLI built a CPU-only app, and
// nobody noticed). Plain `cargo build` stays CPU-only and needs no toolkit. `JUNCTUR_CPU=1`, or any
// features of your own (Tauri's `-f` or cargo's after `--`), builds without it.
// `JUNCTUR_TAURI_DRY=1` prints what would run instead of running it.

import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { createRequire } from "node:module";
import { join } from "node:path";

import { getEnv, ownFeatures, setDefault, withCuda } from "./tauri-args.mjs";

let args = process.argv.slice(2);
const env = { ...process.env };

if (["dev", "build"].includes(args[0])) {
  const cuda = cudaRoot();
  if (getEnv(env, "JUNCTUR_CPU") === "1") {
    say("CPU build (JUNCTUR_CPU=1)");
  } else if (ownFeatures(args)) {
    say("features as given");
  } else if (!cuda) {
    say("CPU build: no CUDA 13 toolkit found (CUDA_PATH_V13_0); Whisper will run on the CPU");
  } else {
    args = withCuda(args);
    // Several CUDA versions installed: CMake would take the first nvcc on PATH.
    setDefault(env, "CUDAToolkit_ROOT", cuda);
    say(`CUDA build (${cuda}); JUNCTUR_CPU=1 for a CPU one`);
  }
  // whisper.cpp is built by CMake, and the cmake crate asks for the newest Visual Studio, which CMake
  // may not know yet (VS 2026 with CMake 4.0); CMake's own default is one it knows.
  if (getEnv(env, "CMAKE_GENERATOR") === undefined) {
    setDefault(env, "CMAKE_GENERATOR", defaultVsGenerator());
  }
}

if (getEnv(env, "JUNCTUR_TAURI_DRY") === "1") {
  const shown = ["CUDAToolkit_ROOT", "CMAKE_GENERATOR"].map((n) => `${n}=${getEnv(env, n) ?? ""}`);
  console.log(JSON.stringify({ args, env: shown }));
  process.exit(0);
}

const require = createRequire(import.meta.url);
const cli = require.resolve("@tauri-apps/cli/tauri.js");
const run = spawnSync(process.execPath, [cli, ...args], { stdio: "inherit", env });
if (run.error) console.error(`junctur: could not run the Tauri CLI: ${run.error.message}`);
process.exit(run.status ?? 1);

function say(line) {
  console.log(`junctur: ${line}`);
}

// The CUDA 13 toolkit folder, when one with nvcc is installed.
function cudaRoot() {
  const nvcc = process.platform === "win32" ? "nvcc.exe" : "nvcc";
  const candidates = [getEnv(env, "CUDA_PATH_V13_0"), getEnv(env, "CUDA_PATH")].filter(
    (d) => d && /v?13(\.\d+)?[\\/]*$/.test(d),
  );
  return candidates.find((d) => existsSync(join(d, "bin", nvcc)));
}

// The starred Visual Studio line of `cmake --help`, or nothing (not Windows, no CMake, not a VS one).
function defaultVsGenerator() {
  if (process.platform !== "win32") return undefined;
  const help = spawnSync("cmake", ["--help"], { encoding: "utf8" });
  const line = (help.stdout ?? "").split(/\r?\n/).find((l) => /^\*\s*Visual Studio/.test(l));
  return line?.replace(/^\*\s*/, "").split("=")[0].replace(/\s*\[arch\]\s*$/, "").trim();
}
