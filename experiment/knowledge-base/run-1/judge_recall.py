import json, subprocess, os, sys, re
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor

KB="/Users/mgorunuch/adi-family/experiment/knowledge-base"
gold=json.load(open(f"{KB}/golden.json"))
COND=sys.argv[1]
rows=json.load(open(f"extracted_{COND}.json"))
by_turn=defaultdict(list)
for r in rows:
    for i,c in enumerate(r["claims"]):
        c["_id"]=f"e{r['turn']}-{i}"
        by_turn[r["turn"]].append(c)

gold_by_turn=defaultdict(list)
for c in gold["claims"]:
    gold_by_turn[c["turn"]].append(c)

SYS = """You score a knowledge-base extractor against a hand-labelled reference.

You get REFERENCE claims (what a careful human recorded from one note) and CANDIDATE claims
(what the extractor produced from that same note).

For each reference claim, decide whether ANY candidate expresses the SAME claim about the
same thing. Judge MEANING, not wording: subject/predicate/value phrasing will differ, and
that is expected and fine. A candidate that says the same thing in other words is a match.
A candidate that is merely on the same topic is NOT a match.

Then check polarity: "+" asserts, "-" rules out. A match with flipped polarity is a
polarity_error — this is the most serious failure, so read it carefully.

Return STRICTLY a JSON array, one object per reference claim, no prose, no markdown fence:

[{"ref":"<reference id>","match":"<candidate id or null>","polarity_ok":true,"note":"<=12 words"}]"""

env={k:v for k,v in os.environ.items() if k not in
     ("CLAUDECODE","CLAUDE_CODE_SESSION_ID","CLAUDE_CODE_CHILD_SESSION",
      "CLAUDE_CODE_MESSAGING_SOCKET","CLAUDE_CODE_MESSAGING_TOKEN","CLAUDE_PID","CLAUDE_EFFORT")}

def fmt(c,i=None):
    return f'{c.get("_id",c.get("id"))}: subject="{c["subject"]}" predicate="{c["predicate"]}" value="{c["value"]}" polarity="{c["polarity"]}"'

def one(turn):
    refs=gold_by_turn[turn]; cands=by_turn.get(turn,[])
    msg=("REFERENCE claims:\n"+"\n".join(fmt(c) for c in refs)+
         "\n\nCANDIDATE claims:\n"+("\n".join(fmt(c) for c in cands) if cands else "(none)"))
    p=subprocess.run(["claude","-p","--model","sonnet","--output-format","json",
                      "--no-session-persistence","--disable-slash-commands","--system-prompt",SYS],
                     input=msg,capture_output=True,text=True,env=env,timeout=300)
    try:
        raw=json.loads(p.stdout).get("result","")
        m=re.search(r"\[.*\]",raw,re.S)
        return json.loads(m.group(0))
    except Exception as e:
        return [dict(ref=c["id"],match=None,polarity_ok=False,note=f"JUDGE FAILED {e}") for c in refs]

turns=sorted(gold_by_turn)
with ThreadPoolExecutor(max_workers=6) as ex:
    out=[x for sub in ex.map(one,turns) for x in sub]
json.dump(out,open(f"recall_{COND}.json","w"),ensure_ascii=False,indent=1)

byid={c["id"]:c for c in gold["claims"]}
hit=[o for o in out if o["match"]]
polerr=[o for o in hit if not o.get("polarity_ok")]
print(f"== {COND}: recall {len(hit)}/{len(out)} = {len(hit)/len(out)*100:.0f}%  polarity errors {len(polerr)}")
print("MISSED:")
for o in out:
    if not o["match"]:
        g=byid[o["ref"]]
        print(f"  {o['ref']} [{g['polarity']}] {g['subject']} | {g['predicate']} | {g['value'][:60]}  -- {o.get('note','')}")
for o in polerr:
    g=byid[o["ref"]]
    print(f"  POLARITY {o['ref']} {g['value'][:50]} -- {o.get('note','')}")
