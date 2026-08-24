import json, subprocess, os, sys, re
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor
gold=json.load(open("golden-flat.json")); COND=sys.argv[1]
rows=json.load(open(f"extracted_{COND}.json"))
by=defaultdict(list)
for r in rows:
    for i,c in enumerate(r["claims"]):
        c["_id"]=f"e{r['turn']}-{i}"; by[r["turn"]].append(c)
gb=defaultdict(list)
for c in gold["claims"]: gb[c["turn"]].append(c)
SYS="""You score a knowledge-base extractor against a hand-labelled reference.

You get REFERENCE facts (what a careful human recorded from one note) and CANDIDATE facts
(what the extractor produced from that same note). Each is one plain sentence.

For each reference fact, decide whether ANY candidate states the SAME fact. Judge MEANING,
not wording. A candidate saying the same thing in other words is a match. A candidate merely
on the same topic is NOT a match. A candidate that states the OPPOSITE — asserting what the
reference rules out, or ruling out what the reference asserts — is not a match either; report
it as reversed.

Return STRICTLY a JSON array, one object per reference fact, no prose, no markdown fence:
[{"ref":"<id>","match":"<candidate id or null>","reversed":false,"note":"<=12 words"}]"""
env={k:v for k,v in os.environ.items() if k not in
 ("CLAUDECODE","CLAUDE_CODE_SESSION_ID","CLAUDE_CODE_CHILD_SESSION","CLAUDE_CODE_MESSAGING_SOCKET",
  "CLAUDE_CODE_MESSAGING_TOKEN","CLAUDE_PID","CLAUDE_EFFORT")}
def one(t):
    refs=gb[t]; cands=by.get(t,[])
    msg=("REFERENCE facts:\n"+"\n".join(f'{c["id"]}: {c["fact"]}' for c in refs)+
         "\n\nCANDIDATE facts:\n"+("\n".join(f'{c["_id"]}: {c["fact"]}' for c in cands) if cands else "(none)"))
    p=subprocess.run(["claude","-p","--model","sonnet","--output-format","json",
        "--no-session-persistence","--disable-slash-commands","--system-prompt",SYS],
        input=msg,capture_output=True,text=True,env=env,timeout=300)
    try:
        raw=json.loads(p.stdout).get("result",""); m=re.search(r"\[.*\]",raw,re.S)
        return json.loads(m.group(0))
    except Exception as e:
        return [dict(ref=c["id"],match=None,reversed=False,note=f"JUDGE FAILED {e}") for c in refs]
with ThreadPoolExecutor(max_workers=6) as ex:
    out=[x for s in ex.map(one,sorted(gb)) for x in s]
hit=[o for o in out if o["match"]]; rev=[o for o in out if o.get("reversed")]
print(f"== {COND}: {len(hit)}/{len(out)} = {len(hit)/len(out)*100:.0f}%   reversed: {len(rev)}")
byid={c["id"]:c for c in gold["claims"]}
for o in out:
    if not o["match"]: print(f"   MISS {o['ref']}: {byid[o['ref']]['fact'][:60]} -- {o.get('note','')}")
