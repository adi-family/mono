import json, urllib.request, itertools, math

KB="/Users/mgorunuch/adi-family/experiment/knowledge-base"
gold=json.load(open(f"{KB}/golden.json"))
claims=gold["claims"]; rels=gold["relations"]
MODEL="mxbai-embed-large"
cache={}

def emb(t):
    if t in cache: return cache[t]
    req=urllib.request.Request("http://127.0.0.1:11434/api/embeddings",
        data=json.dumps({"model":MODEL,"prompt":t}).encode(),
        headers={"Content-Type":"application/json"})
    v=json.loads(urllib.request.urlopen(req,timeout=120).read())["embedding"]
    n=math.sqrt(sum(x*x for x in v)); v=[x/n for x in v]
    cache[t]=v; return v

def cos(a,b): return sum(x*y for x,y in zip(a,b))

for c in claims:
    c["_s"]=emb(c["subject"]); c["_p"]=emb(c["predicate"]); c["_v"]=emb(c["value"])
print(f"embedded {len(cache)} strings with {MODEL}, dim {len(claims[0]['_s'])}")

byid={c["id"]:c for c in claims}
labelled={(r["a"],r["b"]) for r in rels} | {(r["b"],r["a"]) for r in rels}

rows=[]
for a,b in itertools.combinations(claims,2):
    rows.append(dict(a=a["id"],b=b["id"],
        s=cos(a["_s"],b["_s"]), p=cos(a["_p"],b["_p"]), v=cos(a["_v"],b["_v"]),
        pol_same=a["polarity"]==b["polarity"],
        labelled=(a["id"],b["id"]) in labelled))
print(f"{len(rows)} pairs, {sum(r['labelled'] for r in rows)} labelled\n")

# --- Q1: does subject+predicate proximity surface the labelled pairs, and what else? ---
print("Q1 — gate on subject & predicate similarity (pairs that reach the queue)")
print(f"{'thr':>5} {'labelled found':>15} {'total queued':>13} {'noise per real':>15}")
for thr in (0.60,0.65,0.70,0.75,0.80,0.85):
    q=[r for r in rows if r["s"]>=thr and r["p"]>=thr]
    f=sum(r["labelled"] for r in q)
    print(f"{thr:>5.2f} {f:>7}/{len(rels):<7} {len(q):>13} {(len(q)-f)/max(f,1):>15.1f}")

# --- Q2: does value similarity separate contradiction from co-existence? ---
print("\nQ2 — value similarity, by what the mechanical rule must decide")
for kind in ("same-value-opposite-polarity","different-value"):
    vs=sorted(next(r for r in rows if {r["a"],r["b"]}=={x["a"],x["b"]})["v"]
              for x in rels if x["machine"]==kind)
    print(f"  {kind:<32} n={len(vs)} min {vs[0]:.3f} median {vs[len(vs)//2]:.3f} max {vs[-1]:.3f}")

print("\n  every labelled pair, sorted by value similarity:")
for x in sorted(rels,key=lambda x:-next(r for r in rows if {r["a"],r["b"]}=={x["a"],x["b"]})["v"]):
    r=next(r for r in rows if {r["a"],r["b"]}=={x["a"],x["b"]})
    ga,gb=byid[x["a"]],byid[x["b"]]
    flag="CONTRA" if x["machine"]=="same-value-opposite-polarity" else "coexist"
    print(f"    v={r['v']:.3f} s={r['s']:.3f} p={r['p']:.3f}  {flag:<8} {x['verdict']:<10} "
          f"{ga['value'][:34]:<34} [{ga['polarity']}] || {gb['value'][:34]} [{gb['polarity']}]")
json.dump([{k:v for k,v in r.items()} for r in rows],open("pairs.json","w"),indent=1)
