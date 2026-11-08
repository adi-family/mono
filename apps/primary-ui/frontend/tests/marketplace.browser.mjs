/* Marketplace interactions against isolated Chrome. Every API request is intercepted. */
import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { resolve } from "node:path";

const origin = new URL(process.env.SHOWCASE_URL ?? "http://design-system.adi").origin;
const chromePath = process.env.CHROME_BIN ?? (process.platform === "darwin"
  ? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" : "google-chrome");
const profile = await mkdtemp(resolve(tmpdir(), "adi-primary-smoke-"));
const chrome = Bun.spawn([
  chromePath, "--headless=new", "--remote-debugging-port=0", `--user-data-dir=${profile}`,
  "--no-first-run", "--no-default-browser-check", "--disable-background-networking", "about:blank",
], { stdout: "ignore", stderr: "ignore" });

const readPosts = new Set(["/api/agents/run/peek", "/api/agents/run/steps"]);
const errors = [];
const blocked = [];
const interceptionErrors = [];
const requests = [];
const checks = [];
const pause = (ms) => new Promise((done) => setTimeout(done, ms));
const q = (selector) => `document.querySelector(${JSON.stringify(selector)})`;
const grid = 'main[aria-label="Home"] ul[aria-label="Applications"]';
let stage = "startup";
let socket;
let send;
let evaluate;

async function until(fn, description, timeout = 20000) {
  const deadline = Date.now() + timeout;
  let last;
  do {
    try { const result = await fn(); if (result) return result; } catch (error) { last = error; }
    await pause(50);
  } while (Date.now() < deadline);
  throw new Error(`Timed out: ${description}${last ? ` (${last.message})` : ""}`);
}

const pixel = "data:image/svg+xml;base64," + Buffer.from('<svg xmlns="http://www.w3.org/2000/svg" width="100" height="60"><rect width="100" height="60" fill="red"/></svg>').toString("base64");
const fixture = { sources: [{name:"custom",url:"https://example.test/manifest.json"}], apps: [
  {marketplace:"custom",slug:"legacy",name:"Legacy App",description:"Dashboard package with a description that runs past one line.",summary:"One line for the card",version:"1.0",repo:"https://github.com/example/legacy.git",commit:"a".repeat(40),keywords:[],gallery:[{url:pixel,kind:"image",caption:"Preview"},{url:pixel,kind:"image",caption:"Second preview"}],elements:[],installs:[{id:"legacy-copy",name:"My copy",started:false,outdated:true,commit:"b".repeat(40)}],bundle:null},
  {marketplace:"custom",slug:"bundle",name:"Worker Bundle",description:"A scoped bundle",version:"1.0",repo:"https://github.com/example/bundle.git",commit:"a".repeat(40),keywords:[],gallery:[],elements:[{kind:"service",name:"worker"}],installs:[],bundle:{declared:1,elements:[{kind:"services",name:"worker",description:"Background service"}],installs:[{project:"project-one",project_name:"One",commit:"b".repeat(40),outdated:true,missing_secrets:["API_TOKEN"],elements:[{kind:"services",name:"worker",id:"worker",started:false}]},{project:"project-two",project_name:"Two",commit:"b".repeat(40),outdated:true,missing_secrets:[],elements:[{kind:"services",name:"worker",id:"worker",started:false}]}]}}
]};
const mutations=[];
let failNextInstall=false;
async function intercept({requestId,request}) {
  const url=new URL(request.url);
  const path=url.pathname.replace(/^\/api\/app(?=\/)/,"/api");
  requests.push({stage,method:request.method,path});
  let data={}; let status=200;
  if (request.method === "GET" && path === "/api/marketplace") data=fixture;
  else if (request.method === "GET" && path === "/api/projects") data={projects:[{id:"project-one",name:"One"},{id:"project-two",name:"Two"}]};
  else if(request.method === "POST" && path.startsWith("/api/marketplace/")) {
    const body=JSON.parse(request.postData??"null");
    mutations.push({path,body});
    await pause(150);
    if (failNextInstall && path.endsWith("/install")) {failNextInstall=false;status=409;data={error:"Destination already exists"};}
    else {
      if(path.endsWith("/start")) fixture.apps[0].installs[0].started=true;
      if(path.endsWith("/uninstall")) fixture.apps[1].bundle.installs.find(copy=>copy.project===body.project).elements=[];
      data={state:fixture,message:"Action completed"};
    }
  } else if(request.method !== "GET") {blocked.push(request.method+" "+path);status=403;}
  await send("Fetch.fulfillRequest",{requestId,responseCode:status,responseHeaders:[{name:"Content-Type",value:"application/json"}],body:Buffer.from(JSON.stringify(data)).toString("base64")});
}

async function screenshot(name) {
  if (!process.env.BROWSER_SCREENSHOTS_DIR) return;
  await mkdir(process.env.BROWSER_SCREENSHOTS_DIR, { recursive: true });
  const shot = await send("Page.captureScreenshot", { format: "png", captureBeyondViewport: false });
  await writeFile(resolve(process.env.BROWSER_SCREENSHOTS_DIR, `primary-app-${name}.png`), Buffer.from(shot.data, "base64"));
}

try {
  const port = await until(async () => Number((await readFile(resolve(profile, "DevToolsActivePort"), "utf8")).split("\n")[0]), "Chrome debugging port");
  const tabs = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
  socket = new WebSocket(tabs.find((tab) => tab.type === "page").webSocketDebuggerUrl);
  await new Promise((done, reject) => { socket.onopen = done; socket.onerror = reject; });
  let nextId = 1;
  const pending = new Map();
  send = (method, params = {}) => {
    const id = nextId++;
    return new Promise((done, reject) => {
      const timer = setTimeout(() => { pending.delete(id); reject(new Error(`CDP timeout: ${method}`)); }, 15000);
      pending.set(id, { done, reject, timer });
      socket.send(JSON.stringify({ id, method, params }));
    });
  };
  socket.onmessage = (event) => {
    const message = JSON.parse(event.data);
    if (message.id) {
      const waiting = pending.get(message.id);
      if (!waiting) return;
      pending.delete(message.id);
      clearTimeout(waiting.timer);
      message.error ? waiting.reject(new Error(JSON.stringify(message.error))) : waiting.done(message.result);
    }
    if (message.method === "Fetch.requestPaused") void intercept(message.params).catch((error) => {
      // A full document navigation may cancel a read while it is being continued.
      if (!/Invalid InterceptionId|Invalid interceptionId|Invalid requestId|No resource with given identifier|interception.*not found/i.test(error.message)) interceptionErrors.push(error.message);
    });
    let detail;
    if (message.method === "Runtime.exceptionThrown") detail = message.params.exceptionDetails.exception?.description ?? message.params.exceptionDetails.text;
    if (message.method === "Runtime.consoleAPICalled" && message.params.type === "error") detail = message.params.args.map((arg) => arg.value ?? arg.description).join(" ");
    if (message.method === "Log.entryAdded" && message.params.entry.level === "error") detail = message.params.entry.text;
    if (detail && !/Failed to load resource.*409/.test(detail)) errors.push({ stage, detail });
  };
  evaluate = async (expression) => {
    const result = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
    if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text);
    return result.result.value;
  };
  async function check(name, fn) {
    stage = name;
    await fn();
    // Give asynchronously logged render/fetch failures time to arrive.
    await pause(300);
    assert.deepEqual(blocked, [], "no unexpected API writes or cross-origin API requests");
    assert.deepEqual(interceptionErrors, [], "API interception remains active");
    assert.deepEqual(errors, [], "zero browser exceptions or console errors");
    checks.push(name);
    console.log(`PASS ${name}`);
  }
  async function navigate(path, selector) {
    await send("Page.navigate", { url: `${origin}${path}` });
    await until(() => evaluate(`!!${q(selector)}`), `${path} renders ${selector}`);
  }
  async function click(selector) {
    await evaluate(`${q(selector)}.scrollIntoView({block:'center'})`);
    const point = await evaluate(`(() => {const r=${q(selector)}.getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2};})()`);
    await send("Input.dispatchMouseEvent", { type: "mousePressed", button: "left", clickCount: 1, ...point });
    await send("Input.dispatchMouseEvent", { type: "mouseReleased", button: "left", clickCount: 1, ...point });
  }
  async function tokenColor(selector, property = "backgroundColor") {
    const color = await until(() => evaluate(`(() => {const el=${q(selector)};if(!el)return null;const color=getComputedStyle(el)[${JSON.stringify(property)}];return color.startsWith('oklch(')?color:null;})()`), `${selector} applies a theme token`);
    assert.match(color, /^oklch\(/);
  }
  async function settingsLink() {
    assert.equal(await evaluate(`${q('[data-slot="global-navigation"] a[aria-label="Settings"]')}?.href`), `${origin}/extended/settings`);
  }

  await Promise.all([send("Runtime.enable"), send("Page.enable"), send("Log.enable"), send("Network.enable")]);
  await send("Network.setBypassServiceWorker", { bypass: true });
  await send("Fetch.enable", { patterns: [{ urlPattern: "*/api/*", requestStage: "Request" }] });
  await send("Emulation.setDeviceMetricsOverride", { width: 1280, height: 1000, deviceScaleFactor: 1, mobile: false });


  async function value(selector, text) {
    await evaluate(`(() => {const el=${q(selector)};Object.getOwnPropertyDescriptor(el.tagName==='SELECT'?HTMLSelectElement.prototype:HTMLInputElement.prototype,'value').set.call(el,${JSON.stringify(text)});el.dispatchEvent(new Event('input',{bubbles:true}));el.dispatchEvent(new Event('change',{bubbles:true}));})()`);
  }
  async function packagePage(slug) {await navigate(`/marketplace/custom/${slug}`, '[data-slot="marketplace-package"]');}
  async function openInstall() {await click('[data-slot="marketplace-install"]');await until(()=>evaluate(`!!${q('[data-slot="marketplace-install-form"]')}`),'install form opens');}
  async function submitInstall() {await click('[data-slot="marketplace-install-form"] button[type="submit"]');await until(()=>evaluate(`!${q('[data-slot="marketplace-install-form"]')}`),'install succeeds');}
  async function confirmation() {await until(()=>evaluate(`!!${q('dialog[open] button:last-child')}`),'confirmation opens');await click('dialog[open] button:last-child');await until(()=>evaluate(`!${q('dialog[open]')}`),'confirmation completes');}

  await check("React catalog navigates to package and direct reload retains the shell", async()=>{
    await navigate('/marketplace','[data-slot="marketplace-package-link"]');
    assert.equal(await evaluate(`${q('[data-slot="marketplace-package-link"]')}.getAttribute('href')`),'/marketplace/custom/legacy');
    // A card shows the published summary, else the description; the version is a package-page fact.
    assert.deepEqual(await evaluate(`Array.from(document.querySelectorAll('[data-slot="marketplace-summary"]'),line=>line.textContent)`),['One line for the card','A scoped bundle'],'cards prefer the summary');
    assert.equal(await evaluate(`Array.from(document.querySelectorAll('main[aria-label="Marketplace"] [data-slot="card"] *')).some(el=>/^v1\\.0$/.test(el.textContent.trim()))`),false,'cards show no version');
    // The whole card is one link to the package page: no tags, no button, no nested anchors.
    assert.deepEqual(await evaluate(`Array.from(document.querySelectorAll('main[aria-label="Marketplace"] [data-slot="card"]'),card=>({anchors:card.querySelectorAll('a').length,buttons:card.querySelectorAll('button').length,wraps:card.firstElementChild===card.querySelector('[data-slot="marketplace-package-link"]')}))`),[{anchors:1,buttons:0,wraps:true},{anchors:1,buttons:0,wraps:true}],'each card is a single link');
    await evaluate('window.packageNavigationMarker=42');
    await click('[data-slot="marketplace-package-link"]');
    await until(()=>evaluate(`!!${q('[data-slot="marketplace-package"]')}`),'React detail renders');
    assert.equal(await evaluate('window.packageNavigationMarker'),42,'no document redirect');
    assert.equal(await evaluate(`${q('a[aria-label="Marketplace"]')}.getAttribute('aria-current')`),'page');
    await send('Page.reload');
    await until(()=>evaluate(`!!${q('[data-slot="marketplace-package"]')}`),'detail reload');
    await until(()=>evaluate("document.title==='Legacy App — Marketplace — ADI'"),'package document title updates');
    assert(await evaluate(`!!${q('[data-slot="global-navigation"]')}`));
    assert(await evaluate(`Promise.all(Array.from(document.querySelectorAll('[data-slot="marketplace-package"] img'),image=>image.decode())).then(()=>true)`));
    await click('button[aria-label="Next preview"]');
    assert.equal(await evaluate(`${q('[data-slot="marketplace-package-gallery"] figcaption')}.textContent`),'Second preview');
    await click('button[aria-label="Previous preview"]');
    await screenshot('react-detail');
  });
  await check("Legacy install keeps names, destination and start choice; pending prevents duplicates",async()=>{
    await openInstall();
    await value('input[aria-label="Project name"]','New workspace');
    await value('input[aria-label="Copy name"]','Named copy');
    await click('[data-slot="marketplace-install-form"] input[type="checkbox"]');
    await click('[data-slot="marketplace-install-form"] button[type="submit"]');
    assert.equal(await evaluate(`${q('[data-slot="marketplace-install-form"] button[type="submit"]')}.disabled`),true);
    await until(()=>evaluate(`!${q('[data-slot="marketplace-install-form"]')}`),'legacy installation done');
    assert.deepEqual(mutations.at(-1),{path:'/api/marketplace/install',body:{marketplace:'custom',slug:'legacy',element:null,name:'Named copy',start:false,project:null,new_project:'New workspace'}});
  });
  await check("Existing project and global installs use distinct scopes",async()=>{
    await packagePage('bundle');await openInstall();
    await click('input[name="destination"][value="existing"]');
    await until(()=>evaluate(`document.querySelectorAll('select[aria-label="Project"] option').length===3`),'projects load');
    await value('select[aria-label="Project"]','project-two');await submitInstall();
    assert.equal(mutations.at(-1).body.project,'project-two');assert.equal(mutations.at(-1).body.new_project,null);
    await openInstall();await click('input[name="destination"][value="global"]');await submitInstall();
    assert.equal(mutations.at(-1).body.project,null);assert.equal(mutations.at(-1).body.new_project,null);
  });
  await check("Install errors keep the form editable and allow retry",async()=>{
    await openInstall();await value('input[aria-label="Project name"]','Retained destination');failNextInstall=true;
    await click('[data-slot="marketplace-install-form"] button[type="submit"]');
    await until(()=>evaluate(`${q('[data-slot="marketplace-install-form"] [role="alert"]')}?.textContent.includes('Destination already exists')`),'inline install error');
    assert.equal(await evaluate(`${q('input[aria-label="Project name"]')}.value`),'Retained destination');
    await submitInstall();
  });
  await check("Single element uses the directory spelling and Escape cancels without writes",async()=>{
    await click('button[aria-label="Install worker"]');await submitInstall();
    assert.equal(mutations.at(-1).body.element,'services/worker');
    const count=mutations.length;await openInstall();
    await send('Input.dispatchKeyEvent',{type:'keyDown',key:'Escape',code:'Escape',windowsVirtualKeyCode:27});
    await until(()=>evaluate(`!${q('[data-slot="marketplace-install-form"]')}`),'Escape closes install');
    assert.equal(mutations.length,count);
  });
  await check("Start and safe update call existing APIs",async()=>{
    await packagePage('legacy');
    await evaluate(`Array.from(document.querySelectorAll('[data-slot="marketplace-installed"] button')).find(button=>button.textContent==='Start').click()`);
    await until(()=>mutations.at(-1)?.path==='/api/marketplace/start','start request');
    assert.deepEqual(mutations.at(-1).body,{id:'legacy-copy'});
    await until(()=>evaluate(`!Array.from(document.querySelectorAll('[data-slot="marketplace-installed"] button')).some(button=>button.textContent==='Start')`),'started state updates');
    await evaluate(`Array.from(document.querySelectorAll('[data-slot="marketplace-installed"] button')).find(button=>button.textContent==='Update').click()`);
    await until(()=>mutations.at(-1)?.path==='/api/marketplace/update','update request');
    assert.deepEqual(mutations.at(-1).body,{id:'legacy-copy',force:false});
    await until(()=>evaluate(`!${q('[data-slot="marketplace-install"]')}.disabled`),'update settles');
    await evaluate(`Array.from(document.querySelectorAll('[data-slot="marketplace-installed"] button')).find(button=>button.textContent==='Force update…').click()`);
    await confirmation();assert.equal(mutations.at(-1).body.force,true);
  });
  await check("Service and bundle actions retain project and destructive actions require confirmation",async()=>{
    await packagePage('bundle');
    await evaluate(`Array.from(document.querySelectorAll('[data-slot="marketplace-installed"] button')).find(button=>button.textContent==='Start').click()`);
    await until(()=>mutations.at(-1)?.path==='/api/marketplace/start-service','service starts');
    assert.deepEqual(mutations.at(-1).body,{marketplace:'custom',slug:'bundle',name:'worker',project:'project-one'});
    await until(()=>evaluate(`!${q('[data-slot="marketplace-install"]')}.disabled`),'start settles');
    await evaluate(`Array.from(document.querySelectorAll('[data-slot="marketplace-installed"] button')).find(button=>button.textContent==='Update').click()`);
    await until(()=>mutations.at(-1)?.path==='/api/marketplace/bundle/update','bundle update');
    assert.equal(mutations.at(-1).body.project,'project-one');assert.deepEqual(mutations.at(-1).body.force,[]);
    await until(()=>evaluate(`!${q('[data-slot="marketplace-install"]')}.disabled`),'update settles');
    await click('button[aria-label="Force update worker"]');await confirmation();
    assert.deepEqual(mutations.at(-1).body.force,['services/worker']);assert.equal(mutations.at(-1).body.project,'project-one');
    const count=mutations.length;await click('button[aria-label="Uninstall worker"]');
    assert.equal(mutations.length,count,'opening confirmation never uninstalls');
    await confirmation();
    assert.deepEqual(mutations.at(-1),{path:'/api/marketplace/uninstall',body:{marketplace:'custom',slug:'bundle',project:'project-one',element:'services/worker'}});
    assert.equal(await evaluate(`document.querySelectorAll('button[aria-label="Uninstall worker"]').length`),1,'other project is untouched');
    await screenshot('bundle-management');
  });
  await check("Unknown package has an explicit not found state and a way back",async()=>{
    await navigate('/marketplace/custom/missing','main h2');
    assert.equal(await evaluate(`${q('main h2')}.textContent`),'Package not found');
    await click('main a[href="/marketplace"]');
    await until(()=>evaluate(`!!${q('main[aria-label="Marketplace"]')}`),'back to catalog');
  });
  await check("Refresh catalog uses the configured source API",async()=>{
    await evaluate(`Array.from(document.querySelectorAll('main button')).find(button=>button.textContent==='Refresh catalog').click()`);
    await until(()=>mutations.at(-1)?.path==='/api/marketplace/sync','catalog sync request');
    await until(()=>evaluate(`Array.from(document.querySelectorAll('main button')).some(button=>button.textContent==='Refresh catalog'&&!button.disabled)`),'catalog sync settles');
    assert(await evaluate(`document.querySelector('main').textContent.includes('custom')`));
  });
  await check("Mobile package and install dialog fit the viewport",async()=>{
    await send("Emulation.setDeviceMetricsOverride", { width:390, height:844, deviceScaleFactor:1, mobile:true });
    await packagePage('bundle');await openInstall();
    assert(await evaluate(`document.documentElement.scrollWidth<=document.documentElement.clientWidth`),'page does not overflow horizontally');
    assert(await evaluate(`(() => {const r=document.querySelector('dialog[open]').getBoundingClientRect();return r.left>=0&&r.right<=innerWidth&&r.height<=innerHeight;})()`),'dialog fits viewport');
    await screenshot('package-mobile');
    await click('dialog[open] button[type="button"]');
  });
  console.log(`\n${checks.length} marketplace fixture checks passed; all API requests intercepted.`);
} catch (error) {
  console.error(`Primary app smoke failed during ${stage}; ${checks.length} checks completed.`);
  if (errors.length) console.error(`Browser errors: ${JSON.stringify(errors)}`);
  if (blocked.length) console.error(`Blocked API requests: ${JSON.stringify(blocked)}`);
  if (interceptionErrors.length) console.error(`Interception errors: ${JSON.stringify(interceptionErrors)}`);
  console.error(`API request paths: ${JSON.stringify(requests)}`);
  if (evaluate && socket?.readyState === WebSocket.OPEN) {
    try {
      console.error(JSON.stringify(await evaluate(`({path:location.pathname,title:document.title,home:!!${q(grid)},gallery:!!${q('#gallery')},chat:!!${q('[data-slot="app-chat"]')},native:!!${q('.adi-workbench')}})`)));
      await screenshot("failure");
    } catch (diagnosticError) { console.error(`Could not collect diagnostics: ${diagnosticError.message}`); }
  }
  throw error;
} finally {
  socket?.close();
  chrome.kill();
  await chrome.exited;
  await rm(profile, { recursive: true, force: true });
}
