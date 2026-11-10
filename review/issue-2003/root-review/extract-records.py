import hashlib, json, re, sys, zipfile
from pathlib import Path
archive, dest = Path(sys.argv[1]), Path(sys.argv[2])
dest.mkdir(exist_ok=True)
with zipfile.ZipFile(archive) as z:
    names = [n for n in z.namelist() if "Propose statement record" in n]
    assert len(names) == 1, names
    raw = z.read(names[0])
    lines = [re.sub(r"^\ufeff?\d{4}-\d{2}-\d{2}T\S+Z ", "", x) for x in raw.decode("utf-8-sig").splitlines()]
blocks, cur = [], None
for row in lines:
    if re.fullmatch(r"theorem [^ ]+ sha256:[0-9a-f]{64}", row):
        if cur: blocks.append(cur)
        cur = [row]
    elif cur is not None and row.startswith("  "):
        cur.append(row)
    elif cur is not None:
        blocks.append(cur)
        cur = None
if cur: blocks.append(cur)
assert len(blocks) == 249, len(blocks)
assert len({b[0].split()[1] for b in blocks}) == 249
expected = ["perm", "insert_zero", "scale_values", "le_height"]
new = [b for b in blocks if b[0].split()[1].startswith("Botho.Demurrage.ringElapsedCentroid_")]
assert [b[0].split()[1].rsplit("ringElapsedCentroid_",1)[1] for b in new] == expected
for label, selected in [("all", blocks), ("new", new)]:
    data = ("\n\n".join("\n".join(b) for b in selected) + "\n").encode()
    (dest / ("generated-" + label + "-statements.txt")).write_bytes(data)
    print(json.dumps({"kind":label,"blocks":len(selected),"bytes":len(data),"sha256":hashlib.sha256(data).hexdigest()}))
(dest / "records.json").write_text(json.dumps({b[0].split()[1]: b for b in blocks}, indent=2) + "\n")
