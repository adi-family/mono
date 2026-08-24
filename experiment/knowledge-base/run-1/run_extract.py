import json, subprocess, os, sys, re
from concurrent.futures import ThreadPoolExecutor

KB = "/Users/mgorunuch/adi-family/experiment/knowledge-base"
turns = json.load(open(f"{KB}/_human_turns.json"))
sysprompt = open("extract_prompt.txt").read()
CONDITION = sys.argv[1]          # "solo" or "ctx"
MODEL = sys.argv[2] if len(sys.argv) > 2 else "sonnet"

env = {k: v for k, v in os.environ.items()
       if k not in ("CLAUDECODE","CLAUDE_CODE_SESSION_ID","CLAUDE_CODE_CHILD_SESSION",
                    "CLAUDE_CODE_MESSAGING_SOCKET","CLAUDE_CODE_MESSAGING_TOKEN",
                    "CLAUDE_PID","CLAUDE_EFFORT")}

def build(i):
    t = turns[i]
    if CONDITION == "solo":
        return f"NOTE:\n{t['text']}\n"
    prior = turns[max(0, i-2):i]
    ctx = "\n\n".join(f"(earlier, for reference only — do NOT extract from this)\n{p['text']}" for p in prior)
    head = ("CONTEXT — earlier notes from the same conversation. Use them only to resolve what\n"
            "the note below refers to. Extract claims ONLY from the note marked NOTE.\n\n" + ctx + "\n\n") if prior else ""
    return f"{head}NOTE:\n{t['text']}\n"

def one(i):
    t = turns[i]
    p = subprocess.run(
        ["claude","-p","--model",MODEL,"--output-format","json",
         "--no-session-persistence","--disable-slash-commands",
         "--system-prompt", sysprompt],
        input=build(i), capture_output=True, text=True, env=env, timeout=300)
    try:
        d = json.loads(p.stdout)
        raw = d.get("result","")
        cost = d.get("total_cost_usd", 0)
    except Exception:
        return dict(turn=t["seq"], error=p.stdout[:300] + p.stderr[:300], claims=[], cost=0)
    m = re.search(r"\[.*\]", raw, re.S)
    try:
        claims = json.loads(m.group(0)) if m else []
    except Exception:
        return dict(turn=t["seq"], error="unparseable: " + raw[:300], claims=[], cost=cost)
    for c in claims:
        c["turn"] = t["seq"]
    return dict(turn=t["seq"], claims=claims, cost=cost)

with ThreadPoolExecutor(max_workers=6) as ex:
    res = list(ex.map(one, range(len(turns))))

out = f"extracted_{CONDITION}.json"
json.dump(res, open(out,"w"), ensure_ascii=False, indent=1)
n = sum(len(r["claims"]) for r in res)
errs = [r for r in res if r.get("error")]
print(f"{CONDITION}: {n} claims from {len(res)} turns, cost ${sum(r['cost'] for r in res):.2f}, errors {len(errs)}")
for e in errs[:3]:
    print("  ERR turn", e["turn"], e["error"][:160])
