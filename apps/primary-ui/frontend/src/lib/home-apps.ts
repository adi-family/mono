import {
  Activity, Blocks, BookOpen, Bot, Braces, Bug, Code, FlaskConical,
  HeartPulse, MessageCircleQuestion, MessageSquare, ShieldCheck, Terminal, Video, Wallet, Zap,
  type LucideIcon,
} from "lucide-react";

import { appApiBase } from "./app-api";
import { browserStorage, readSaved, writeSaved } from "./saved-results";

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
  /** Restored from this browser's saved catalog, not read from the machine on this visit. */
  saved?: boolean;
};

const icons: Record<string, LucideIcon> = {
  activity: Activity, blocks: Blocks, "book-open": BookOpen, bot: Bot, braces: Braces,
  bug: Bug, code: Code, "flask-conical": FlaskConical, "heart-pulse": HeartPulse,
  "message-circle-question": MessageCircleQuestion, "message-square": MessageSquare,
  "shield-check": ShieldCheck, terminal: Terminal, video: Video, wallet: Wallet, zap: Zap,
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

async function request(path: string, signal?: AbortSignal): Promise<unknown> {
  const response = await fetch(`${appApiBase()}${path}`, { signal });
  let payload: unknown;
  try { payload = await response.json(); }
  catch { throw new Error("ADI returned an unreadable response. Try again."); }
  if (!response.ok || object(payload) && payload.ok === false) {
    throw new Error(object(payload) && text(payload.error) || "The request could not be completed. Try again.");
  }
  return payload;
}

/*
 * The last catalogs this browser read (lib/saved-results). Home opens with them and refreshes in
 * the background, so a visit starts with its apps instead of an empty grid that fills in as each
 * read answers.
 *
 * What is kept is the server's own rows, cut down to the fields the parsers above read, and a
 * saved catalog goes through the same parser as a live one. Nothing in storage can therefore
 * become a link the server could not have sent, and a machine that was locked or unreachable
 * when it was last read comes back exactly that way, without apps.
 */
export const homeCatalogKeys = { local: "adi.home-apps.local.v1", fleet: "adi.home-apps.fleet.v1" } as const;
type CatalogKind = keyof typeof homeCatalogKeys;

const localFields = ["id", "name", "description", "icon", "host", "archived_at", "frontend_running", "frontend_port"];
const remoteFields = ["id", "name", "description", "icon", "url", "allowed"];
const keep = (value: unknown, fields: string[]) => object(value)
  ? Object.fromEntries(fields.filter((field) => Object.hasOwn(value, field)).map((field) => [field, value[field]])) : null;
const keepRows = (rows: unknown, fields: string[]) => Array.isArray(rows) ? rows.map((row) => keep(row, fields)) : null;

function saveCatalog(kind: CatalogKind, payload: JsonObject, storage: Storage | undefined, now: number) {
  const catalog = kind === "local" ? { dashboards: keepRows(payload.dashboards, localFields) }
    : { nodes: (payload.nodes as unknown[]).map((node) => object(node) ? {
      ...keep(node, ["node", "locked", "error"]),
      // The parser lists no apps for a locked or failing machine, so none are kept for one.
      dashboards: node.locked === true || text(node.error) ? [] : keepRows(node.dashboards, remoteFields),
    } : null) };
  writeSaved(homeCatalogKeys[kind], catalog, storage, now);
}

const savedCatalog = <T>(kind: CatalogKind, parse: (payload: unknown) => T, storage: Storage | undefined) => readSaved(homeCatalogKeys[kind], parse, storage);

export function saveLocalApps(payload: unknown, storage = browserStorage(), now = Date.now()) {
  const apps = localApps(payload);
  saveCatalog("local", payload as JsonObject, storage, now);
  return apps;
}
export function saveFleetMachines(payload: unknown, storage = browserStorage(), now = Date.now()) {
  const machines = fleetMachines(payload);
  saveCatalog("fleet", payload as JsonObject, storage, now);
  return machines;
}
export const savedLocalApps = (storage = browserStorage()) => savedCatalog("local", localApps, storage);
export const savedFleetMachines = (storage = browserStorage()) =>
  savedCatalog("fleet", (payload) => fleetMachines(payload).map((machine): HomeMachine => ({ ...machine, saved: true })), storage);

export async function getLocalApps(signal?: AbortSignal) { return saveLocalApps(await request("/dashboards", signal)); }
export async function getFleetMachines(signal?: AbortSignal) { return saveFleetMachines(await request("/fleet/dashboards", signal)); }
