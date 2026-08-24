from kb import KB
def show(kb, title):
    st = kb.stale()
    print(f"\n{title}")
    if not st: print("   всё свежее"); return
    for s in st:
        print(f"   ПРОТУХЛО  глубина {s['depth']}  {s['id']}: {s['fact'][:58]}")
        print(f"             причина: {s['root_cause']}")

kb = KB()
# --- ground truth: the operator said these, an agent wrote them down ---
kb.add("f-aud-1", "Our target audience is founders who cannot afford to hire a large team but want to delegate.",
       author="igor", creator="agent:extractor@1")
kb.add("f-aud-2", "Solo developers are part of our target audience.", author="igor", creator="agent:extractor@1")
kb.add("f-tone", "We want to be a professional tool, not a growth-hack toy.", author="igor", creator="agent:extractor@1")

# --- predictions: an agent built these on top ---
kb.add("d-profile", "Audience profile: technically capable solo builders, no hiring budget, want to delegate.",
       author="agent:profiler@2", creator="agent:profiler@2", sources=["f-aud-1", "f-aud-2"])
kb.add("d-hero", "Hero: 'Delegate like you have a team. Because now you do.'",
       author="agent:copy@3", creator="agent:copy@3", sources=["d-profile", "f-tone"])
kb.add("d-meta", "Meta description built from the hero line.",
       author="agent:copy@3", creator="agent:copy@3", sources=["d-hero"])

show(kb, "1. Только что всё собрано:")

print("\n2. Игорь правит ОДИН факт про ЦА (добавляет второй сегмент)...")
kb.edit("f-aud-1", "Our target audience is founders who cannot afford to hire a large team, and also small agencies.")
show(kb, "   что стало:")

print("\n3. Агент перегенерировал профиль и пере-штамповал его источники...")
kb.refresh("d-profile")
show(kb, "   что осталось:")

print("\n4. Перегенерировали hero, затем meta...")
kb.refresh("d-hero"); kb.refresh("d-meta")
show(kb, "   итог:")

print("\n5. Правим тон — заденет hero и meta, но НЕ профиль (он от тона не зависит):")
kb.edit("f-tone", "We want to be a professional tool, and we never joke in copy.")
show(kb, "   что стало:")
