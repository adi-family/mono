// The C surface of adi-mesh-ffi. See crates/adi-mesh-ffi/src/lib.rs for the contract.
//
// Every function returns a NUL-terminated JSON string that the CALLER OWNS and must release with
// adi_mesh_free. The shape is always one of:
//
//     {"ok": true,  "value": <anything>}
//     {"ok": false, "error": "a sentence for a human"}
//
// Call adi_mesh_start once before anything else; every other function answers with an `ok: false`
// error until then, and none of them ever traps.

#ifndef ADI_MESH_H
#define ADI_MESH_H

#ifdef __cplusplus
extern "C" {
#endif

// Bind the endpoint and begin accepting pairings. `home` may be NULL to use the process's own
// $HOME, which on iOS is already the app container. value: {"key": "<this device's endpoint id>"}
char *adi_mesh_start(const char *home);

// Close the endpoint and stop the tasks. Safe when never started. value: {"stopped": true}
char *adi_mesh_stop(void);

// value: {"key": "<endpoint id>", "ready": <bool>} — `ready` is false until the relay session is
// up, i.e. until an invite would name something a node can actually dial.
char *adi_mesh_status(void);

// Mint a single-use invite for a node to spend with `adi-mono mesh join <token>`.
// value: {"token": "adi-invite:…"}
char *adi_mesh_invite(void);

// Spend an invite a MACHINE minted (`adi-mono mesh invite`), pairing from this side — the mirror
// of adi_mesh_invite, and the direction that works when the person holding the phone is not also
// sitting at the machine. Carries the plaintext password ONCE, exactly as adi_mesh_take_pairings
// does: put it in the Keychain, because only a digest of it is kept anywhere else.
// value: {"petname": "<what this device now calls it>", "username": "…", "password": "…"}
char *adi_mesh_join(const char *token);

// value: [{"petname", "key", "nickname", "paired_at", "pending_nickname", "services", "any_service"}]
char *adi_mesh_nodes(void);

// Bind (or reuse) the loopback port serving `service` on `node`. value: {"port": <number>}
char *adi_mesh_open(const char *node, const char *service);

// The dashboards `node` publishes, asked of its control panel over the mesh with the credential
// this device holds for it. `allowed` says whether this device's grants already cover the row;
// `service` is NULL for a dashboard that declares no <label>.adi host and so cannot be opened.
// value: {"me": "<what the node calls this device>" | null,
//         "dashboards": [{"id", "name", "description", "service", "running", "allowed"}]}
char *adi_mesh_dashboards(const char *node, const char *username, const char *password);

// Ask `node` to grant this device `http:<service>` — how a dashboard with "allowed": false
// becomes one this device may open. value: {"petname": "<what the node calls this device>"}
char *adi_mesh_allow(const char *node, const char *service, const char *username,
                     const char *password);

// Drain pairings completed since the last call; each is returned EXACTLY ONCE and carries the
// plaintext password, which belongs in the Keychain and nowhere else.
// value: [{"petname", "username", "password"}]
char *adi_mesh_take_pairings(void);

// Unpair a node locally. value: {"removed": <bool>}
char *adi_mesh_forget(const char *node);

// Retire pooled connections after the app returns to the foreground. value: {"resumed": true}
char *adi_mesh_resume(void);

// Release a string returned by any of the above. NULL is a no-op.
void adi_mesh_free(char *reply);

#ifdef __cplusplus
}
#endif

#endif // ADI_MESH_H
