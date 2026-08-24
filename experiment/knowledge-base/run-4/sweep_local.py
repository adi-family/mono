import json, urllib.request, time, os, re
SYS=open('sweep_all.py').read().split('SYS="""')[1].split('"""')[0]
facts=json.load(open("base_facts.json")); byid={f["id"]:f for f in facts}
pairs=json.load(open("all_pairs.json"))
CK="sweep_local_ck.json"
done=json.load(open(CK)) if os.path.exists(CK) else {}
B=60
def mk(chunk):
    return "\n\n".join(
        f'[{i}] A (note {byid[p["a"]]["turn"]}): {byid[p["a"]]["fact"]}\n'
        f'     B (note {byid[p["b"]]["turn"]}): {byid[p["b"]]["fact"]}' for i,p in chunk)
idx=[(i,p) for i,p in enumerate(pairs) if str(i) not in done]
chunks=[idx[i:i+B] for i in range(0,len(idx),B)]
t0=time.time()
for n,ch in enumerate(chunks):
    body={"model":"qwen3.6","system":SYS,"prompt":mk(ch),"stream":False,"think":False,
          "options":{"temperature":0,"num_ctx":16384}}
    try:
        req=urllib.request.Request("http://127.0.0.1:11434/api/generate",
            data=json.dumps(body).encode(),headers={"Content-Type":"application/json"})
        raw=json.loads(urllib.request.urlopen(req,timeout=1800).read())["response"]
        got=json.loads(raw[raw.index("["):raw.rindex("]")+1])
    except Exception as e:
        # one retry per chunk; a parse failure is usually a stray fence or a truncated tail
        try:
            got=[json.loads(m.group(0)) for m in re.finditer(r'\{[^{}]*"verdict"[^{}]*\}',raw)]
        except Exception:
            got=[]
    for g in got:
        if isinstance(g,dict) and "i" in g: done[str(g["i"])]=g
    json.dump(done,open(CK,"w"))
    el=time.time()-t0
    print(f"{n+1}/{len(chunks)}  собрано {len(done)}/{len(pairs)}  {el/60:.1f} мин  осталось ~{el/(n+1)*(len(chunks)-n-1)/60:.0f} мин",flush=True)
out=[]
for i,p in enumerate(pairs):
    g=done.get(str(i),{"verdict":"MISSING","why":""})
    a,b=byid[p["a"]],byid[p["b"]]
    out.append(dict(a=p["a"],b=p["b"],s=p["s"],rank=i+1,verdict=g.get("verdict","MISSING"),
                    why=g.get("why",""),same_note=a["turn"]==b["turn"],ta=a["turn"],tb=b["turn"]))
json.dump(out,open("all_pair_verdicts_local.json","w"),ensure_ascii=False,indent=1)
print("ГОТОВО")
