"""Admission gate for widened paraphrase cases.

A case is admitted when at most 0.34 of the question's content stems also occur in the target
node's label and body, and the target id resolves in the graph. A question over that bar is a
keyword case in a longer sentence. Usage: gate.py <graph.json> <cases.jsonl>
"""
import json, re, sys
import snowballstemmer

RU = snowballstemmer.stemmer("russian")
EN = snowballstemmer.stemmer("english")
STOP = set("""и в во не что он на я с со как а то все она так его но да ты к у же вы за бы по только ее мне было
вот от меня еще нет о из ему теперь когда даже ну вдруг ли если уже или ни быть был него до вас нибудь опять уж
вам ведь там потом себя ничего ей может они тут где есть надо ней для мы тебя их чем была сам чтоб без будто чего
раз тоже себе под будет ж тогда кто этот того потому этого какой совсем ним здесь этом один почти мой тем чтобы нее
сейчас были куда зачем всех никогда можно при наконец два об другой хоть после над больше тот через эти нас про
всего них какая много разве три эту моя впрочем хорошо свою этой перед иногда лучше чуть том нельзя такой им более
всегда конечно всю между это сразу какие ему нужно сколько чьего чьё the a an of to and or in on for is""".split())
MAX = 0.34

def stems(text):
    out = set()
    for w in re.findall(r"[\w]+", text.lower()):
        if w in STOP or len(w) < 2 or w.isdigit():
            continue
        out.add((RU if re.search("[а-яё]", w) else EN).stemWord(w))
    return out

def main(graph, cases):
    nodes = json.load(open(graph))["nodes"]
    nodes = nodes.values() if isinstance(nodes, dict) else nodes
    by = {n["id"]: n for n in nodes}
    bad = 0
    for line in open(cases):
        if not line.strip():
            continue
        c = json.loads(line)
        n = by.get(c["expect"])
        if n is None:
            print(f"REJECT {c['expect']}: anchor does not resolve"); bad += 1; continue
        q = stems(c["q"])
        shared = q & stems(n["label"] + " " + n["body"])
        r = len(shared) / max(len(q), 1)
        verdict = "ok    " if r <= MAX else "REJECT"
        bad += r > MAX
        print(f"{verdict} {c['expect']:<11} {r:.2f}  shared={sorted(shared)}")
    sys.exit(1 if bad else 0)

if __name__ == "__main__":
    main(*sys.argv[1:])
