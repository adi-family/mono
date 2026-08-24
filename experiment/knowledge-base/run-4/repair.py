import json, urllib.request, re, time
SYS=open('sweep_all.py').read().split('SYS="""')[1].split('"""')[0]
facts=json.load(open("base_facts.json")); byid={f["id"]:f for f in facts}
out=json.load(open("all_pair_verdicts_local.json"))
miss=[o for o in out if o["verdict"] in ("MISSING","ERROR",None)]
print(f"добиваем {len(miss)}",flush=True)
B=10
for k in range(0,len(miss),B):
    ch=miss[k:k+B]
    prompt="\n\n".join(
      f'[{j}] A (note {byid[o["a"]]["turn"]}): {byid[o["a"]]["fact"]}\n'
      f'     B (note {byid[o["b"]]["turn"]}): {byid[o["b"]]["fact"]}' for j,o in enumerate(ch))
    body={"model":"qwen3.6","system":SYS,"prompt":prompt,"stream":False,"think":False,
          "options":{"temperature":0,"num_ctx":8192}}
    try:
        req=urllib.request.Request("http://127.0.0.1:11434/api/generate",
            data=json.dumps(body).encode(),headers={"Content-Type":"application/json"})
        raw=json.loads(urllib.request.urlopen(req,timeout=900).read())["response"]
        got=[json.loads(m.group(0)) for m in re.finditer(r'\{[^{}]*"verdict"[^{}]*\}',raw)]
    except Exception as e:
        got=[]
    for g in got:
        j=g.get("i")
        if isinstance(j,int) and 0<=j<len(ch):
            ch[j]["verdict"]=g.get("verdict","MISSING"); ch[j]["why"]=g.get("why","")
    print(f"  {min(k+B,len(miss))}/{len(miss)}",flush=True)
json.dump(out,open("all_pair_verdicts_local.json","w"),ensure_ascii=False,indent=1)
from collections import Counter
print(Counter(o["verdict"] for o in out).most_common())
