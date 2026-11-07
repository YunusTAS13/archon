#!/usr/bin/env python3
"""limine.h icindeki istek kimliklerini Rust sabitlerine cevirir.

Limine istek struct'lari 4 adet u64 sihirli sayi ile baslar. Bu sayilari elle
yazmak yaygin bir hata kaynagidir: tek bir yanlis bit, bootloader'in istegi
tamamen yok saymasina ve kernelin erken panic etmesine yol acar.
"""

import re
import sys

HEADER = """// @generated tools/gen_limine_ids.py tarafindan limine.h'den uretildi.
// Elle duzenlemeyin; `make limine-ids` yeniden uretir.

"""


def main() -> int:
    if len(sys.argv) != 3:
        print("kullanim: gen_limine_ids.py limine.h cikti.rs", file=sys.stderr)
        return 2

    header_path, output_path = sys.argv[1], sys.argv[2]
    source = open(header_path, encoding="utf-8").read()

    common = re.search(r"#\s*define\s+LIMINE_COMMON_MAGIC\s+([^\n/]+)", source)
    if not common:
        print("LIMINE_COMMON_MAGIC bulunamadi", file=sys.stderr)
        return 1
    common_values = [v.strip() for v in common.group(1).split(",") if v.strip()]

    requests = {}
    pattern = r"#\s*define\s+(LIMINE_\w+_REQUEST(?:_ID)?)\s+\{([^}]*)\}"
    for match in re.finditer(pattern, source):
        name, body = match.group(1), match.group(2)
        if name.endswith("_ID"):
            name = name[:-3]
        body = body.replace("LIMINE_COMMON_MAGIC", ", ".join(common_values))
        values = [v.strip() for v in body.split(",") if v.strip()]
        if len(values) != 4:
            continue
        if not all(v.lower().startswith("0x") for v in values):
            continue
        requests[name] = values

    if not requests:
        print("hicbir istek kimligi bulunamadi", file=sys.stderr)
        return 1

    with open(output_path, "w", encoding="utf-8") as out:
        out.write(HEADER)
        out.write("#![allow(dead_code)]\n\n")
        for name in sorted(requests):
            values = ", ".join(requests[name])
            out.write(f"pub const {name}: [u64; 4] = [{values}];\n")

    print(f"{len(requests)} istek kimligi yazildi: {output_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
