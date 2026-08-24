import json, urllib.request, itertools, math
KB="/Users/mgorunuch/adi-family/experiment/knowledge-base"
gold=json.load(open(f"{KB}/golden.json")); claims=gold["claims"]; rels=gold["relations"]
cache={}
def emb(t,model="mxbai-embed-large"):
    k=(model,t)
    if k in cache: return cache[k]
    req=urllib.request.Request("http://127.0.0.1:11434/api/embeddings",
        data=json.dumps({"model":model,"prompt":t}).encode(),headers={"Content-Type":"application/json"})
    v=json.loads(urllib.request.urlopen(req,timeout=120).read())["embedding"]
    n=math.sqrt(sum(x*x for x in v)); v=[x/n for x in v]; cache[k]=v; return v
def cos(a,b): return sum(x*y for x,y in zip(a,b))

for c in claims:
    c["_s"]=emb(c["subject"]); c["_p"]=emb(c["predicate"])
    c["_addr"]=emb(f'{c["subject"]} — {c["predicate"]}')      # address as ONE string
labelled={frozenset((r["a"],r["b"])) for r in rels}

print("Q3 — gate on the ADDRESS embedded as one string, vs two separate fields\n")
print(f"{'thr':>5} | {'2-field: found':>14} {'queued':>7} | {'1-string: found':>15} {'queued':>7}")
for thr in (0.50,0.55,0.60,0.65,0.70,0.75,0.80):
    two=[p for p in itertools.combinations(claims,2) if cos(p[0]["_s"],p[1]["_s"])>=thr and cos(p[0]["_p"],p[1]["_p"])>=thr]
    one=[p for p in itertools.combinations(claims,2) if cos(p[0]["_addr"],p[1]["_addr"])>=thr]
    f2=sum(frozenset((a["id"],b["id"])) in labelled for a,b in two)
    f1=sum(frozenset((a["id"],b["id"])) in labelled for a,b in one)
    print(f"{thr:>5.2f} | {f2:>7}/{len(rels):<6} {len(two):>7} | {f1:>8}/{len(rels):<6} {len(one):>7}")

print("\nQ4 — confirmation load: pairs a human must resolve as claims arrive one by one")
for thr in (0.60,0.70,0.80):
    for field,key in (("2-field",None),("1-string","_addr")):
        tot=0; per=[]
        for i,c in enumerate(claims):
            if key: n=sum(1 for prev in claims[:i] if cos(c[key],prev[key])>=thr)
            else:   n=sum(1 for prev in claims[:i] if cos(c["_s"],prev["_s"])>=thr and cos(c["_p"],prev["_p"])>=thr)
            per.append(n); tot+=n
        print(f"  thr {thr:.2f} {field:<9} total {tot:>4} confirmations for {len(claims)} claims "
              f"= {tot/len(claims):.1f} per claim, worst single claim {max(per)}")
print(f"\n  the corpus produced ~5.2 claims per dictated turn, so multiply by ~5 for load per turn.")
