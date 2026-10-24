// Embedded guest console (RFC 0023): xterm.js for terminals (containers,
// VMs on serial0), noVNC otherwise. Homie opens the console and relays the
// WebSocket, so the Proxmox ticket never reaches the browser. Loaded as its
// own chunk: the other pages do not carry noVNC or xterm.js.

import RFB from "@novnc/novnc";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { Link, useParams } from "@tanstack/react-router";
import { useEffect, useRef, useState } from "react";
import { act } from "../api";
import { Badge, Button } from "../ui";

type Mode = "vnc" | "term";
type Opened = { id: string; guest: string; mode: Mode; switchable: boolean; password: string | null };
type Report = (text: string, live: boolean) => void;

function vnc(el: HTMLElement, url: string, password: string | null, report: Report, keep: (r: RFB | null) => void) {
  const credentials = { password: password ?? "" };
  const r = new RFB(el, url, { credentials, wsProtocols: ["binary"] });
  r.scaleViewport = true;
  r.addEventListener("connect", () => report("connected", true));
  r.addEventListener("disconnect", (e) => report((e as CustomEvent<{ clean: boolean }>).detail.clean ? "closed" : "connection lost", false));
  r.addEventListener("credentialsrequired", () => r.sendCredentials(credentials));
  keep(r);
  return () => {
    keep(null);
    r.disconnect();
  };
}

// termproxy framing: "0:<bytes>:<data>" input, "1:<cols>:<rows>:" resize, "2" ping
function term(el: HTMLElement, url: string, report: Report) {
  const t = new Terminal({ cursorBlink: true, fontSize: 13, fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace" });
  const fit = new FitAddon();
  t.loadAddon(fit);
  t.open(el);
  const ws = new WebSocket(url, ["binary"]);
  ws.binaryType = "arraybuffer";
  const enc = new TextEncoder();
  const send = (s: string) => ws.readyState === WebSocket.OPEN && ws.send(s);
  const resize = () => {
    fit.fit();
    send(`1:${t.cols}:${t.rows}:`);
  };
  ws.onopen = () => {
    report("connected", true);
    resize();
    t.focus();
  };
  ws.onmessage = (e) => t.write(typeof e.data === "string" ? e.data : new Uint8Array(e.data as ArrayBuffer));
  ws.onclose = (e) => {
    report(e.reason ? `closed: ${e.reason}` : "closed", false);
    t.write("\r\n[console closed]\r\n");
  };
  t.onData((d) => send(`0:${enc.encode(d).length}:${d}`));
  const ping = window.setInterval(() => send("2"), 30_000);
  const observer = new ResizeObserver(resize);
  observer.observe(el);
  return () => {
    window.clearInterval(ping);
    observer.disconnect();
    ws.close();
    t.dispose();
  };
}

export function Console() {
  const { pve, node, type, vmid } = useParams({ strict: false }) as Record<string, string>;
  // none: Homie picks (a terminal when the guest has one)
  const [mode, setMode] = useState<Mode | undefined>(undefined);
  const [attempt, setAttempt] = useState(0);
  const [opened, setOpened] = useState<Opened | null>(null);
  const [status, setStatus] = useState({ text: "connecting", live: false });
  const [error, setError] = useState("");
  const screen = useRef<HTMLDivElement>(null);
  const rfb = useRef<RFB | null>(null);

  useEffect(() => {
    let gone = false;
    let close = () => {};
    setError("");
    setStatus({ text: "connecting", live: false });
    const report: Report = (text, live) => !gone && setStatus({ text, live });
    act<Opened>(`/api/act/pve/${encodeURIComponent(pve)}/console`, { node, vmid: Number(vmid), type, mode })
      .then((o) => {
        if (gone || !screen.current) return;
        setOpened(o);
        const url = `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/api/act/pve/${encodeURIComponent(pve)}/console/${o.id}`;
        close = o.mode === "vnc" ? vnc(screen.current, url, o.password, report, (r) => (rfb.current = r)) : term(screen.current, url, report);
      })
      .catch((e: Error) => !gone && setError(e.message));
    return () => {
      gone = true;
      close();
      screen.current?.replaceChildren();
    };
  }, [pve, node, type, vmid, mode, attempt]);

  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center gap-3 text-sm">
        <Link to="/infra" className="text-ink-subtle hover:text-ink">
          ← Infrastructure
        </Link>
        <span className="font-medium">{opened?.guest ?? `${type} ${vmid}`}</span>
        <span className="text-ink-subtle">
          {node}, {opened?.mode === "vnc" ? "VNC" : opened ? "terminal" : ""}
        </span>
        <Badge tone={status.live ? "ok" : "muted"}>{status.text}</Badge>
        <span className="ml-auto flex gap-2">
          {opened?.mode === "vnc" && status.live && <Button onClick={() => rfb.current?.sendCtrlAltDel()}>Ctrl+Alt+Del</Button>}
          {opened?.switchable && <Button onClick={() => setMode(opened.mode === "vnc" ? "term" : "vnc")}>{opened.mode === "vnc" ? "Use serial terminal" : "Use VNC"}</Button>}
          {!status.live && <Button onClick={() => setAttempt((n) => n + 1)}>Reconnect</Button>}
        </span>
      </div>
      {error && <p className="text-sm text-bad">{error}</p>}
      <div ref={screen} className="h-[calc(100vh-11rem)] min-h-80 overflow-hidden rounded-control border border-hairline bg-black" />
    </div>
  );
}
