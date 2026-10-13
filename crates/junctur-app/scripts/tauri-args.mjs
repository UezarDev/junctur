// The pure half of `tauri.mjs`: which features the user already chose, where `cuda` goes, and
// environment defaults that respect Windows' case-insensitive variable names. Tested by
// `node --test scripts/tauri-args.test.mjs`.

// `tauri dev|build [tauri options] -- [cargo args] -- [app args]` (the second `--` only for `dev`).
// True when the user picked features themselves, as a Tauri option or a cargo one.
export function ownFeatures(args) {
  const first = args.indexOf("--");
  const tauri = first < 0 ? args : args.slice(0, first);
  const rest = first < 0 ? [] : args.slice(first + 1);
  const second = rest.indexOf("--");
  const cargo = second < 0 ? rest : rest.slice(0, second);
  // Tauri: -f / --features, also `-f=x`, `-fx`, `--features=x`.
  const inTauri = tauri.slice(1).some((a) => /^(-f|--features)(=|$)/.test(a) || /^-f[^-]/.test(a));
  // cargo: -F / --features (same forms), and --all-features.
  const inCargo = cargo.some(
    (a) => /^(-F|--features|--all-features)(=|$)/.test(a) || /^-F[^-]/.test(a),
  );
  return inTauri || inCargo;
}

// `args` with `cuda` turned on, right after the subcommand: past a `--` it would reach cargo or the
// app instead. One token, so the multi-value `--features` cannot swallow a positional after it.
export function withCuda(args) {
  return [args[0], "--features=cuda", ...args.slice(1)];
}

// The key `name` is stored under in `env` (any casing on Windows, where `CUDATOOLKIT_ROOT` and
// `CUDAToolkit_ROOT` are one variable), or undefined.
export function envKey(env, name, platform = process.platform) {
  if (name in env) return name;
  if (platform !== "win32") return undefined;
  const lower = name.toLowerCase();
  return Object.keys(env).find((k) => k.toLowerCase() === lower);
}

// Sets `name` only when no spelling of it is set, so the child never gets two of them.
export function setDefault(env, name, value, platform = process.platform) {
  if (value === undefined || envKey(env, name, platform) !== undefined) return;
  env[name] = value;
}

// The value of `name` under any spelling (see `envKey`).
export function getEnv(env, name, platform = process.platform) {
  const k = envKey(env, name, platform);
  return k === undefined ? undefined : env[k];
}
