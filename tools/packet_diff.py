#!/usr/bin/env python3
# packet_diff.py — diff decompiled Java packet classes between two client trees.
# Compares two decompiled Java source trees (e.g. MC 1.21.1 vs MC 26.3) by
# protocol packet class name suffix, printing a unified diff per class pair.
#
# packet_diff.py — дифф декомпилированных Java-деревьев по классам протокольных
# пакетов (например, 1.21.1 против 26.3), поиск по суффиксу имени класса.
#
# Usage / Использование:
#   python tools/packet_diff.py --old <DIR> --new <DIR> --filter ClientboundCustomPayload
#   python tools/packet_diff.py --old <DIR> --new <DIR> --filter ServerboundMovePlayer --max-diff 120
#
# stdlib-only / только stdlib. CRLF-safe / устойчив к CRLF.

import argparse
import difflib
import sys
from pathlib import Path

# Verdict labels printed per class pair.
# Вердикты, печатаемые для каждой пары классов.
IDENTICAL = "IDENTICAL"
CHANGED = "CHANGED"
ONLY_IN_OLD = "ONLY_IN_OLD"
ONLY_IN_NEW = "ONLY_IN_NEW"


def read_lines(path: Path) -> list[str]:
    """Read a Java file, normalize line endings, strip trailing whitespace.

    Читает .java файл: нормализует переводы строк (CRLF/LF -> LF)
    и убирает концевые пробелы, чтобы дифф не шумел от форматирования.
    Возвращает list строк без терминаторов.
    """
    # errors="replace": decompilers occasionally emit odd bytes; never crash.
    # errors="replace": декомпиляторы иногда выдают странные байты; не падаем.
    text = path.read_text(encoding="utf-8", errors="replace")
    lines = text.replace("\r\n", "\n").replace("\r", "\n").split("\n")
    # Normalize whitespace runs inside lines too, so indentation style
    # (tabs vs spaces, alignment padding) does not pollute the diff.
    # Также схлопываем внутренние пробельные последовательности, чтобы стиль
    # отступов (табы/пробелы, выравнивание) не засорял дифф.
    return [" ".join(line.split()) for line in lines]


def find_sources(root: Path, name_filter: str) -> dict[str, Path]:
    """Recursively collect .java files whose file name contains the filter.

    Рекурсивно ищет .java файлы, чьё имя файла содержит подстроку filter
    (например "ClientboundCustomPayload"). Возвращает словарь
    {имя_файла: путь}. MCP-obfuscated package names are irrelevant here:
    matching is by simple file-name suffix/substring.
    MCP-обфусцированные имена пакетов не важны: ищем по имени файла.
    """
    result: dict[str, Path] = {}
    for path in root.rglob("*.java"):
        if name_filter.lower() in path.name.lower():
            # First occurrence wins on duplicates; keep it deterministic.
            # При дублях берём первый по порядку обхода — детерминизм.
            result.setdefault(path.name, path)
    return result


def diff_pair(old_path: Path, new_path: Path, max_lines: int) -> tuple[str, list[str]]:
    """Compare one old/new file pair. Returns (verdict, diff lines).

    Сравнивает пару файлов old/new (построчно, с нормализацией пробелов,
    difflib.unified_diff). Возвращает (вердикт, список строк диффа
    длиной не более max_lines).
    """
    old_lines = read_lines(old_path)
    new_lines = read_lines(new_path)

    diff = list(
        difflib.unified_diff(
            old_lines,
            new_lines,
            fromfile=f"old/{old_path.name}",
            tofile=f"new/{new_path.name}",
            lineterm="",
        )
    )
    if not diff:
        return IDENTICAL, []

    # Drop the two header lines before truncation, keep pure hunks.
    # Отрезаем две заголовочные строки перед обрезкой — оставляем ханки.
    hunks = diff[2:]
    if len(hunks) > max_lines:
        hunks = hunks[:max_lines]
        hunks.append(f"... [diff truncated: {len(diff) - 2} hunk lines total]")
    return CHANGED, hunks


def main() -> int:
    parser = argparse.ArgumentParser(
        description=(
            "Diff decompiled Java packet classes between two client trees / "
            "Дифф пакетных классов между двумя декомпилированными деревьями"
        )
    )
    parser.add_argument("--old", required=True, help="old tree root / корень старого дерева")
    parser.add_argument("--new", required=True, help="new tree root / корень нового дерева")
    parser.add_argument(
        "--filter",
        required=True,
        help="class-name substring / подстрока имени класса (напр. ClientboundCustomPayload)",
    )
    parser.add_argument(
        "--max-diff",
        type=int,
        default=60,
        help="max diff lines printed per class (default 60) / максимум строк диффа на класс",
    )
    args = parser.parse_args()

    old_root = Path(args.old)
    new_root = Path(args.new)
    for label, root in (("old", old_root), ("new", new_root)):
        if not root.is_dir():
            print(f"ERROR: --{label} directory not found: {root}", file=sys.stderr)
            return 2

    old_files = find_sources(old_root, args.filter)
    new_files = find_sources(new_root, args.filter)

    if not old_files and not new_files:
        print(f"No .java files matching filter '{args.filter}' in either tree.")
        print(f"Ни одного .java файла с подстрокой '{args.filter}' не найдено.")
        return 1

    counts = {IDENTICAL: 0, CHANGED: 0, ONLY_IN_OLD: 0, ONLY_IN_NEW: 0}

    # Union of file names, sorted for deterministic output.
    # Объединение имён файлов, отсортированное для детерминированного вывода.
    all_names = sorted(set(old_files) | set(new_files))
    for name in all_names:
        old_path = old_files.get(name)
        new_path = new_files.get(name)
        if old_path and new_path:
            verdict, diff_lines = diff_pair(old_path, new_path, args.max_diff)
            counts[verdict] += 1
            rel_old = old_path.relative_to(old_root)
            print(f"\n=== {name} [{verdict}] ===")
            print(f"    old: {rel_old}")
            if diff_lines:
                print("\n".join(diff_lines))
        elif old_path:
            counts[ONLY_IN_OLD] += 1
            print(f"\n=== {name} [{ONLY_IN_OLD}] ===")
            print(f"    old: {old_path.relative_to(old_root)}")
        else:
            counts[ONLY_IN_NEW] += 1
            print(f"\n=== {name} [{ONLY_IN_NEW}] ===")
            print(f"    new: {new_path.relative_to(new_root)}")

    # Summary / сводка.
    total = len(all_names)
    print("\n" + "=" * 60)
    print(f"SUMMARY for filter '{args.filter}' ({total} class(es))")
    print(f"  IDENTICAL   : {counts[IDENTICAL]}")
    print(f"  CHANGED     : {counts[CHANGED]}")
    print(f"  ONLY_IN_OLD : {counts[ONLY_IN_OLD]}")
    print(f"  ONLY_IN_NEW : {counts[ONLY_IN_NEW]}")
    print("=" * 60)
    return 0


if __name__ == "__main__":
    sys.exit(main())
