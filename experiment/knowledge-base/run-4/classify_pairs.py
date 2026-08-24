import json, subprocess, os, re
from concurrent.futures import ThreadPoolExecutor
facts={f["id"]:f for f in json.load(open("base_facts.json"))}
pairs=json.load(open("base_top_pairs.json"))
SYS="""You are reviewing a knowledge base built from one person's dictated notes. You get two
facts that the base found similar. Say what a reviewer would have to do about them.

  duplicate    — they say the same thing; one merged fact would replace both
  narrows      — one is a more specific or qualified version of the other; still compatible
  independent  — both true at once, about different things; nothing to do
  controversy  — they cannot comfortably both stand. One rules out or reverses the other, or
                 the person changed his mind. A reviewer MUST look at this.

Be strict about `controversy`. Two facts about the same topic are not a controversy. A hope
and a doubt about the same thing IS one. A decision reversed later IS one.

Return STRICTLY JSON, no prose, no fence: {"verdict":"...","why":"<=15 words"}"""
env={k:v for k,v in os.environ.items() if k not in
 ("CLAUDECODE","CLAUDE_CODE_SESSION_ID","CLAUDE_CODE_CHILD_SESSION","CLAUDE_CODE_MESSAGING_SOCKET",
  "CLAUDE_CODE_MESSAGING_TOKEN","CLAUDE_PID","CLAUDE_EFFORT")}
def one(p):
    a,b=facts[p["a"]],facts[p["b"]]
    msg=f'A (note {a["turn"]}): {a["fact"]}\nB (note {b["turn"]}): {b["fact"]}'
    r=subprocess.run(["claude","-p","--model","sonnet","--output-format","json",
        "--no-session-persistence","--disable-slash-commands","--system-prompt",SYS],
        input=msg,capture_output=True,text=True,env=env,timeout=300)
    try:
        raw=json.loads(r.stdout).get("result","")
        d=json.loads(re.search(r"\{.*\}",raw,re.S).group(0))
    except Exception as e:
        d={"verdict":"ERROR","why":str(e)[:40]}
    return {**p, **d, "same_note": a["turn"]==b["turn"]}
with ThreadPoolExecutor(max_workers=8) as ex:
    out=list(ex.map(one,pairs))
json.dump(out,open("base_pair_verdicts.json","w"),ensure_ascii=False,indent=1)
from collections import Counter
print("верхние 120 пар базы, что с ними делать:")
for k,v in Counter(o["verdict"] for o in out).most_common(): print(f"  {k:<12} {v}")
print("\nразрез: пары внутри одной заметки vs между заметками")
for sn in (True,False):
    c=Counter(o["verdict"] for o in out if o["same_note"]==sn)
    print(f"  {'внутри одной' if sn else 'между'}: {dict(c)}")
print("\nCONTROVERSY:")
for o in out:
    if o["verdict"]=="controversy":
        a,b=facts[o["a"]],facts[o["b"]]
        print(f"  {o['s']:.3f} [{a['turn']}] {a['fact'][:70]}")
        print(f"         [{b['turn']}] {b['fact'][:70]}")
        print(f"         -> {o['why']}")
