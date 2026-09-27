// Windows process helpers for the app end-to-end test: they only list processes, counters and
// TCP connections (read-only), except `killTree`, which is only ever called with the pid of the
// process the test itself started.
import { execFileSync } from "node:child_process";
import { existsSync, readdirSync } from "node:fs";
import { join } from "node:path";

/** Runs a PowerShell script (Windows PowerShell 5.1 is on every Windows machine and runner). */
function ps(script) {
  const quiet = `$ProgressPreference = 'SilentlyContinue'\n${script}`;
  const encoded = Buffer.from(quiet, "utf16le").toString("base64");
  return execFileSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-EncodedCommand", encoded], {
    encoding: "utf8",
    maxBuffer: 16 * 1024 * 1024,
    windowsHide: true,
    stdio: ["ignore", "pipe", "pipe"],
  });
}

/** Pids of running processes with this image name (without `.exe`). */
export function pidsNamed(name) {
  const out = ps(`@(Get-Process -Name '${name}' -ErrorAction SilentlyContinue | ForEach-Object { $_.Id }) -join ','`);
  return out.trim().split(",").filter(Boolean).map(Number);
}

/**
 * The process tree below `rootPid` (WebView2 runs as its descendants) with each process's
 * private working set, the root's main window handle, and the tree's established TCP
 * connections.
 */
export function inspectTree(rootPid) {
  const out = ps(`
    $ErrorActionPreference = 'SilentlyContinue'
    $all = @(Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, Name)
    $tree = [System.Collections.Generic.List[int]]::new()
    $tree.Add(${Number(rootPid)})
    for ($i = 0; $i -lt $tree.Count; $i++) {
      $all | Where-Object { $_.ParentProcessId -eq $tree[$i] -and $_.ProcessId -ne $tree[$i] } |
        ForEach-Object { $tree.Add([int]$_.ProcessId) }
    }
    $perf = @(Get-CimInstance Win32_PerfFormattedData_PerfProc_Process |
      Where-Object { $tree -contains [int]$_.IDProcess } |
      Select-Object @{n='pid';e={[int]$_.IDProcess}}, @{n='name';e={$_.Name}}, @{n='private';e={[double]$_.WorkingSetPrivate}})
    $tcp = @(Get-NetTCPConnection -State Established |
      Where-Object { $tree -contains [int]$_.OwningProcess } |
      Select-Object LocalAddress, LocalPort, RemoteAddress, RemotePort, @{n='pid';e={[int]$_.OwningProcess}})
    $root = Get-Process -Id ${Number(rootPid)}
    [pscustomobject]@{
      pids = @($tree)
      processes = $perf
      tcp = $tcp
      mainWindow = if ($root) { [int64]$root.MainWindowHandle } else { 0 }
      alive = [bool]$root
    } | ConvertTo-Json -Depth 4 -Compress
  `);
  const info = JSON.parse(out);
  const list = (v) => (v == null ? [] : Array.isArray(v) ? v : [v]);
  return {
    pids: list(info.pids),
    processes: list(info.processes),
    tcp: list(info.tcp),
    mainWindow: Number(info.mainWindow) || 0,
    alive: Boolean(info.alive),
    privateBytes: list(info.processes).reduce((sum, p) => sum + (p.private || 0), 0),
  };
}

/** Whether any of `pids` is still running. */
export function anyAlive(pids) {
  if (!pids.length) return false;
  const out = ps(`@(Get-Process -Id ${pids.map(Number).join(",")} -ErrorAction SilentlyContinue).Count`);
  return Number(out.trim()) > 0;
}

/** Force-ends the tree of a process this test started (never anything else). */
export function killTree(pid) {
  try {
    execFileSync("taskkill.exe", ["/PID", String(pid), "/T", "/F"], { stdio: "ignore", windowsHide: true });
  } catch {
    // already gone
  }
}

const LOOPBACK = new Set(["127.0.0.1", "::1", "::ffff:127.0.0.1"]);

/** Established connections other than the test's own DevTools socket on `debugPort`. */
export function unexpectedConnections(tcp, debugPort) {
  return tcp.filter(
    (c) => !(isLoopbackPair(c) && (c.LocalPort === debugPort || c.RemotePort === debugPort)) && !isAppOrigin(c),
  );
}

/**
 * The app's own origin, `http://tauri.localhost` (loopback port 80). WebView2 serves it from memory,
 * but it occasionally lets a request reach the real network stack; the socket never leaves the
 * machine (seen on CI runners that listen on port 80). Reported by the harness, not failed.
 */
export function isAppOrigin(c) {
  return isLoopbackPair(c) && c.RemotePort === 80;
}

function isLoopbackPair(c) {
  return LOOPBACK.has(c.LocalAddress) && LOOPBACK.has(c.RemoteAddress);
}

/** Relative names of every entry below `dir` (sorted), or null when it does not exist. */
export function listNames(dir, skip = () => false) {
  if (!existsSync(dir)) return null;
  const names = [];
  const walk = (sub) => {
    let entries;
    try {
      entries = readdirSync(join(dir, sub), { withFileTypes: true });
    } catch {
      return; // unreadable or removed meanwhile
    }
    for (const e of entries) {
      const rel = sub ? `${sub}\\${e.name}` : e.name;
      if (skip(rel)) continue;
      names.push(rel);
      if (e.isDirectory()) walk(rel);
    }
  };
  walk("");
  return names.sort();
}
