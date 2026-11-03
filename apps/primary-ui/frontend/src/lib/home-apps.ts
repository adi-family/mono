import {
  Activity, Blocks, BookOpen, Bot, Braces, Bug, FlaskConical,
  HeartPulse, MessageCircleQuestion, MessageSquare, ShieldCheck, Video, Wallet, Zap,
  type LucideIcon,
} from "lucide-react";

import { appApiBase } from "./app-api";

export type HomeApp = {
  id: string;
  key: string;
  name: string;
  description: string;
  source: string;
  href: string | null;
  icon: LucideIcon | null;
  notice?: string;
};

export type HomeMachine = {
  id: string;
  label: string;
  locked: boolean;
  apps: HomeApp[];
  issue?: string;
  href?: string;
};

const icons: Record<string, LucideIcon> = {
  activity: Activity, blocks: Blocks, "book-open": BookOpen, bot: Bot, braces: Braces,
  bug: Bug, "flask-conical": FlaskConical, "heart-pulse": HeartPulse,
  "message-circle-question": MessageCircleQuestion, "message-square": MessageSquare,
  "shield-check": ShieldCheck, video: Video, wallet: Wallet, zap: Zap,
};
const defaultIcons: Record<string, LucideIcon> = {
  adin: Bot, "agent-board": MessageSquare, "ask-the-founder": MessageCircleQuestion,
  "automate-this": Zap, "company-handbook": BookOpen, "debt-ledger": Wallet,
  "defence-security-operation": ShieldCheck, "design-system": Blocks, "jev-lab": FlaskConical,
  "llm-gateway": Activity, pulse: HeartPulse, recorder: Video, "vite-starter": Braces,
};
type JsonObject = Record<string, unknown>;
const object = (value: unknown): value is JsonObject => typeof value === "object" && value !== null && !Array.isArray(value);
const text = (value: unknown) => typeof value === "string" ? value.trim() : "";
const dnsLabel = /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/;
const validHost = (value: string) => value.length <= 253 && value.split(".").every((label) => dnsLabel.test(label));

function appInfo(value: unknown, source: string): HomeApp | null {
  if (!object(value) || !text(value.id)) return null;
  const id = text(value.id);
  return {
    id, key: JSON.stringify([source, id]), source, name: text(value.name) || id,
    description: text(value.description), href: null,
    icon: Object.hasOwn(icons, text(value.icon)) ? icons[text(value.icon)] : Object.hasOwn(defaultIcons, id) ? defaultIcons[id] : null,
  };
}

export function localApps(payload: unknown): HomeApp[] {
  if (!object(payload) || !Array.isArray(payload.dashboards)) throw new Error("The app catalog could not be read.");
  return payload.dashboards.flatMap((value) => {
    const app = appInfo(value, "");
    if (!app || !object(value) || value.archived_at != null) return [];
    const host = text(value.host).toLowerCase();
    if (host && validHost(host)) app.href = `http://${host}/`;
    else if (value.frontend_running === true && Number.isInteger(value.frontend_port) && Number(value.frontend_port) > 0 && Number(value.frontend_port) <= 65535) {
      app.href = `http://127.0.0.1:${value.frontend_port}/`;
    }
    // Only this showcase's own tile enters its local gallery. Remote copies open their node.
    if (app.id === "design-system" && host === "design-system.adi") app.href = "/library";
    if (!app.href) app.notice = "No app address is available.";
    return [app];
  });
}

function remoteUrl(value: unknown, node: string): string | null {
  try {
    const url = new URL(text(value));
    if (!["http:", "https:"].includes(url.protocol) || url.username || url.password ||
      !validHost(url.hostname) || !url.hostname.endsWith(`.${node}.n.adi`)) return null;
    return url.href;
  } catch { return null; }
}

export function fleetMachines(payload: unknown): HomeMachine[] {
  if (!object(payload) || !Array.isArray(payload.nodes)) throw new Error("The machines’ app catalog could not be read.");
  return payload.nodes.flatMap((value) => {
    if (!object(value) || !dnsLabel.test(text(value.node))) return [];
    const node = text(value.node);
    const href = `http://app.${node}.n.adi/`;
    const locked = value.locked === true;
    const issue = locked ? text(value.error) || "Unlock this machine to see its apps." : text(value.error) ||
      (!Array.isArray(value.dashboards) ? "This machine’s app catalog could not be read." : undefined);
    const apps = issue ? [] : (Array.isArray(value.dashboards) ? value.dashboards : []).flatMap((row) => {
      const app = appInfo(row, node);
      if (!app || !object(row)) return [];
      const url = remoteUrl(row.url, node);
      if (!url) app.notice = "No app address is available.";
      else if (row.allowed === true) app.href = url;
      else { app.href = href; app.notice = "Access required · Open machine"; }
      return [app];
    });
    return [{ id: node, label: node, locked, apps, issue, href }];
  });
}

async function request(path: string, signal?: AbortSignal, body?: JsonObject): Promise<unknown> {
  const response = await fetch(`${appApiBase()}${path}`, {
    signal,
    ...(body ? { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) } : {}),
  });
  let payload: unknown;
  try { payload = await response.json(); }
  catch { throw new Error("ADI returned an unreadable response. Try again."); }
  if (!response.ok || object(payload) && payload.ok === false) {
    throw new Error(object(payload) && text(payload.error) || "The request could not be completed. Try again.");
  }
  return payload;
}

export async function getLocalApps(signal?: AbortSignal) { return localApps(await request("/dashboards", signal)); }
export async function getFleetMachines(signal?: AbortSignal) { return fleetMachines(await request("/fleet/dashboards", signal)); }

function requireNode(node: string) {
  if (!dnsLabel.test(node)) throw new Error("Choose a valid paired machine.");
}

export async function unlockMachine(node: string, username: string, password: string, signal?: AbortSignal) {
  requireNode(node);
  if (!password.length) throw new Error("Enter the machine password.");
  return fleetMachines(await request("/fleet/dashboards/unlock", signal, { node, username: username.trim() || "adi", password }));
}

export async function lockMachine(node: string, signal?: AbortSignal) {
  requireNode(node);
  return fleetMachines(await request("/fleet/dashboards/forget", signal, { petname: node }));
}
