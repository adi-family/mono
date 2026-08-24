import json, urllib.request, urllib.error, math, itertools, subprocess, time, os, sys
TOKEN=subprocess.run(["gcloud","auth","application-default","print-access-token"],
                     capture_output=True,text=True).stdout.strip()
PROJ="mono-504617"
CACHE="vertex_cache.json"
cache=json.load(open(CACHE)) if os.path.exists(CACHE) else {}

def vertex(model, texts, task, dim=None):
    url=(f"https://us-central1-aiplatform.googleapis.com/v1/projects/{PROJ}"
         f"/locations/us-central1/publishers/google/models/{model}:predict")
    out=[]
    for t in texts:
        key=f"{model}|{task}|{dim}|{t}"
        if key in cache: out.append(cache[key]); continue
        body={"instances":[{"task_type":task,"content":t}]}
        if dim: body["parameters"]={"outputDimensionality":dim}
        delay=3
        for attempt in range(8):
            req=urllib.request.Request(url,data=json.dumps(body).encode(),
                headers={"Authorization":f"Bearer {TOKEN}","Content-Type":"application/json"})
            try:
                d=json.loads(urllib.request.urlopen(req,timeout=90).read())
                v=d["predictions"][0]["embeddings"]["values"]
                n=math.sqrt(sum(x*x for x in v)) or 1.0
                v=[x/n for x in v]; cache[key]=v; out.append(v)
                json.dump(cache,open(CACHE,"w"))
                time.sleep(1.2)   # the 429s came fast; pace every call, not just retries
                break
            except urllib.error.HTTPError as e:
                if e.code in (429,503) and attempt<7:
                    time.sleep(delay); delay=min(delay*2,60); continue
                raise
    return out

def cos(a,b): return sum(x*y for x,y in zip(a,b))
f=json.load(open('golden-flat.json')); cl=f['claims']; rels=f['relations']
lab={frozenset((r["a"],r["b"])) for r in rels}
byid={c["id"]:c for c in cl}; facts=[c["fact"] for c in cl]

def score(name, vecs, show=False):
    es={c["id"]:v for c,v in zip(cl,vecs)}
    pairs=[dict(a=a["id"],b=b["id"],st=cos(es[a["id"]],es[b["id"]]),
                lab=frozenset((a["id"],b["id"])) in lab) for a,b in itertools.combinations(cl,2)]
    r=sorted(pairs,key=lambda x:-x["st"])
    at=[sum(1 for x in r[:K] if x["lab"]) for K in (20,30,50,80,120)]
    worst=max(i for i,x in enumerate(r) if x["lab"])+1
    ch=next(x for x in pairs if {x["a"],x["b"]}=={"mkt-05","mkt-07"})
    print(f"{name:<50} {at[0]:>2} {at[1]:>3} {at[2]:>3} {at[3]:>3} {at[4]:>4}   топ-{worst:<4} Китай:{r.index(ch)+1}",flush=True)
    if show:
        print("\nТОП-15 очереди:")
        for x in r[:15]:
            a,b=byid[x["a"]],byid[x["b"]]
            print(f"{'✓' if x['lab'] else ' '} {x['st']:.3f}  {a['fact'][:62]}")
            print(f"          {b['fact'][:62]}")
    return r

print(f"{'модель / task_type':<50} {'@20':>2} {'@30':>3} {'@50':>3} {'@80':>3} {'@120':>4}   все 14",flush=True)
print("-"*102,flush=True)
runs=[("gemini-embedding-001 (3072d) SEMANTIC_SIMILARITY","gemini-embedding-001","SEMANTIC_SIMILARITY",None),
      ("gemini-embedding-001 (3072d) RETRIEVAL_DOCUMENT","gemini-embedding-001","RETRIEVAL_DOCUMENT",None),
      ("gemini-embedding-001 (3072d) CLUSTERING","gemini-embedding-001","CLUSTERING",None),
      ("gemini-embedding-001 (768d)  SEMANTIC_SIMILARITY","gemini-embedding-001","SEMANTIC_SIMILARITY",768),
      ("text-multilingual-embedding-002 SEM_SIM","text-multilingual-embedding-002","SEMANTIC_SIMILARITY",None),
      ("text-embedding-005 SEM_SIM","text-embedding-005","SEMANTIC_SIMILARITY",None)]
best=None
for name,model,task,dim in runs:
    r=score(name, vertex(model,facts,task,dim), show=False)
    if best is None: best=r
print()
score("gemini-embedding-001 (3072d) SEMANTIC_SIMILARITY",
      vertex("gemini-embedding-001",facts,"SEMANTIC_SIMILARITY"), show=True)
