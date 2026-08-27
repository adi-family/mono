// IndexedDB, as three functions.
//
// This is the client's whole persistence layer: the browser's iroh secret key and one record per
// paired node — endpoint id, relay, petname, username, password. It never leaves this origin, and
// there is nowhere else for it to go; a browser has no Keychain, so clearing site data destroys
// the pairing and that is a property of the design, not a bug to work around.
//
// Why IndexedDB and not localStorage, which would be a tenth of the code: localStorage is
// synchronous, capped at a few megabytes of *strings*, and — the reason that settles it — is not
// available inside a service worker. The map from a panel's client id to the node it belongs to is
// written by the page and read by the worker (see `sw.js`), and IndexedDB is the only store both
// of them can see.
//
// Written in JavaScript rather than through `web-sys` because IndexedDB's API is event-based
// callbacks all the way down: the Rust version is the same logic four times over in `Closure`s,
// and this file is the shape the API actually has.

const DB = "adi-mesh-client";
const STORE = "kv";
const VERSION = 1;

let open;

/** The database, opened once and shared. */
function db() {
  if (!open) {
    open = new Promise((resolve, reject) => {
      const request = indexedDB.open(DB, VERSION);
      request.onupgradeneeded = () => {
        if (!request.result.objectStoreNames.contains(STORE)) {
          request.result.createObjectStore(STORE);
        }
      };
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error);
      // A second tab holding an old version open blocks the upgrade. There is only one version so
      // far, so this can only fire on a downgrade — but a hang with no message is the worst answer.
      request.onblocked = () => reject(new Error("another tab is holding the database open"));
    });
  }
  return open;
}

function run(mode, work) {
  return db().then(
    (database) =>
      new Promise((resolve, reject) => {
        const tx = database.transaction(STORE, mode);
        const request = work(tx.objectStore(STORE));
        request.onsuccess = () => resolve(request.result);
        request.onerror = () => reject(request.error);
      }),
  );
}

/** The string stored under `key`, or null. */
export async function load(key) {
  const value = await run("readonly", (store) => store.get(key));
  return value === undefined ? null : value;
}

/** Store `value` under `key`. */
export async function save(key, value) {
  await run("readwrite", (store) => store.put(value, key));
}

/** Forget `key`. */
export async function forget(key) {
  await run("readwrite", (store) => store.delete(key));
}
