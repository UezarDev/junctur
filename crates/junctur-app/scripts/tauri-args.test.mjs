// `node --test scripts/tauri-args.test.mjs`: the wrapper's argument and environment rules (2l).

import assert from "node:assert/strict";
import { test } from "node:test";

import { envKey, ownFeatures, setDefault, withCuda } from "./tauri-args.mjs";

test("features the user chose, in every place and form, count", () => {
  for (const args of [
    ["dev", "-f", "x"],
    ["dev", "-f=x"],
    ["dev", "-fx"],
    ["build", "--features", "x"],
    ["build", "--features=x"],
    ["build", "--", "--features", "x"],
    ["build", "--", "-F", "x"],
    ["build", "--", "-Fx"],
    ["build", "--", "--features=x"],
    ["dev", "--", "--all-features", "--", "file.mp4"],
  ]) {
    assert.equal(ownFeatures(args), true, args.join(" "));
  }
});

test("app arguments and other options are not features", () => {
  for (const args of [
    ["dev"],
    ["build", "--debug"],
    ["dev", "--", "--", "--features", "file.mp4"],
    ["dev", "--", "--", "-F"],
    ["build", "--", "--release"],
  ]) {
    assert.equal(ownFeatures(args), false, args.join(" "));
  }
});

test("cuda goes right after the subcommand, as one token", () => {
  assert.deepEqual(withCuda(["dev", "--", "--", "file.mp4"]), [
    "dev",
    "--features=cuda",
    "--",
    "--",
    "file.mp4",
  ]);
});

test("a Windows variable set in another casing is kept, not doubled", () => {
  const env = { CUDATOOLKIT_ROOT: "old" };
  setDefault(env, "CUDAToolkit_ROOT", "new", "win32");
  assert.deepEqual(env, { CUDATOOLKIT_ROOT: "old" });
  assert.equal(envKey(env, "CUDAToolkit_ROOT", "linux"), undefined);
  const unset = {};
  setDefault(unset, "CMAKE_GENERATOR", "Visual Studio 16 2019", "win32");
  setDefault(unset, "CUDAToolkit_ROOT", undefined, "win32");
  assert.deepEqual(unset, { CMAKE_GENERATOR: "Visual Studio 16 2019" });
});
