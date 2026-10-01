#!/usr/bin/env python3
"""Generate the 200-project SDK-style solution for brief 0002.

Layout (deterministic, seed 2002):
  * 160 class libraries in 7 layers (L0..L6). Every project in layer k > 0
    references one project in layer k-1 (so layer 6 is 6 reference levels
    deep) plus up to two more from any lower layer.
  * 40 xunit.v3 test projects, each referencing one or two libraries.
  * About 50 source files per project.
  * Packages come only from the local NuGet cache (see nuget.config written
    next to the solution), so restore and load are offline.

Usage: generate.py [--out DIR] [--files-per-project N]
Writes DIR/Bench200.slnx and DIR/probe.json (the completion probe position).
"""
from __future__ import annotations

import argparse
import json
import os
import random
import shutil
from pathlib import Path

LAYER_SIZES = [30, 28, 26, 24, 22, 18, 12]  # 160 libraries, 7 layers (depth 6)
TEST_PROJECTS = 40
XUNIT_V3 = "4.0.1"
DI_ABSTRACTIONS = "10.0.10"
HUMANIZER = "2.14.1"


def lib_name(layer: int, idx: int) -> str:
    return f"Bench.L{layer}.P{idx:02d}"


def write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def lib_csproj(refs: list[str], packages: list[tuple[str, str]]) -> str:
    items = "".join(f'    <ProjectReference Include="../{r}/{r}.csproj" />\n' for r in refs)
    pkgs = "".join(f'    <PackageReference Include="{n}" Version="{v}" />\n' for n, v in packages)
    return (
        '<Project Sdk="Microsoft.NET.Sdk">\n'
        "  <ItemGroup>\n" + items + pkgs + "  </ItemGroup>\n"
        "</Project>\n"
    )


def test_csproj(refs: list[str]) -> str:
    items = "".join(f'    <ProjectReference Include="../{r}/{r}.csproj" />\n' for r in refs)
    return (
        '<Project Sdk="Microsoft.NET.Sdk">\n'
        "  <PropertyGroup>\n"
        "    <OutputType>Exe</OutputType>\n"
        "    <IsTestProject>true</IsTestProject>\n"
        "  </PropertyGroup>\n"
        "  <ItemGroup>\n"
        f'    <PackageReference Include="xunit.v3" Version="{XUNIT_V3}" />\n'
        + items
        + "  </ItemGroup>\n"
        "</Project>\n"
    )


def widget_source(ns: str, j: int, dep: str | None, rng: random.Random, uses_di: bool, uses_humanizer: bool) -> str:
    dep_field = f"    private readonly global::{dep}.Widget{rng.randrange(50):02d} _dep = new();\n" if dep else ""
    compute = "_dep.Compute(x) + Value" if dep else f"x * {j + 1} + Value"
    usings = "using System.Collections.Generic;\nusing System.Linq;\n"
    if uses_di:
        usings += "using Microsoft.Extensions.DependencyInjection;\n"
    if uses_humanizer:
        usings += "using Humanizer;\n"
    di = (
        "    public static IServiceCollection Register(IServiceCollection services)\n"
        f"        => services.AddSingleton<Widget{j:02d}>();\n\n"
        if uses_di
        else ""
    )
    describe = '        => Name.Humanize() + " " + Value.ToWords();\n' if uses_humanizer else '        => $"{Name} ({Value})";\n'
    return f"""{usings}
namespace {ns};

/// <summary>Generated type {j} in {ns}.</summary>
public interface IWidget{j:02d}
{{
    int Value {{ get; }}
    int Compute(int x);
}}

public sealed record Widget{j:02d}Options(string Label, int Seed, bool Enabled);

public sealed class Widget{j:02d} : IWidget{j:02d}
{{
{dep_field}    public int Value {{ get; set; }} = {j};

    public string Name => "Widget{j:02d}";

    public Widget{j:02d}Options Options {{ get; init; }} = new("w{j}", {j}, true);

    public int Compute(int x) => {compute};

    public IEnumerable<int> Range(int count)
    {{
        for (var i = 0; i < count; i++)
        {{
            yield return Compute(i);
        }}
    }}

    public int Sum(int count) => Range(count).Where(v => v % 2 == 0).Sum();

    public string Describe()
{describe}
{di}    public override string ToString() => $"{{Name}}:{{Value}}";
}}
"""


def test_source(ns: str, j: int, target: str, rng: random.Random) -> str:
    w = rng.randrange(50)
    return f"""using Xunit;

namespace {ns};

public sealed class Widget{w:02d}Tests{j:02d}
{{
    [Fact]
    public void Compute_IsDeterministic()
    {{
        var widget = new global::{target}.Widget{w:02d}();
        Assert.Equal(widget.Compute({j}), widget.Compute({j}));
    }}

    [Theory]
    [InlineData(1)]
    [InlineData({j + 2})]
    public void Range_HasRequestedLength(int count)
    {{
        var widget = new global::{target}.Widget{w:02d}();
        Assert.Equal(count, System.Linq.Enumerable.Count(widget.Range(count)));
    }}
}}
"""


def main() -> None:
    here = Path(__file__).resolve().parent
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=str(here / "out"))
    ap.add_argument("--files-per-project", type=int, default=50)
    args = ap.parse_args()
    out = Path(args.out).resolve()
    files = args.files_per_project
    rng = random.Random(2002)

    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)

    cache = Path(os.environ.get("NUGET_PACKAGES", Path.home() / ".nuget" / "packages"))
    write(out / "nuget.config", f"""<?xml version="1.0" encoding="utf-8"?>
<configuration>
  <packageSources>
    <clear />
    <add key="local-cache" value="{cache}" />
  </packageSources>
</configuration>
""")
    write(out / "Directory.Build.props", """<Project>
  <PropertyGroup>
    <TargetFramework>net10.0</TargetFramework>
    <Nullable>enable</Nullable>
    <ImplicitUsings>enable</ImplicitUsings>
    <LangVersion>latest</LangVersion>
    <GenerateDocumentationFile>false</GenerateDocumentationFile>
  </PropertyGroup>
</Project>
""")
    # Stop MSBuild from walking up into the repository's own props/targets.
    write(out / "Directory.Build.targets", "<Project />\n")
    write(out / "Directory.Packages.props", "<Project />\n")

    projects: list[str] = []
    layers: list[list[str]] = []
    deps: dict[str, list[str]] = {}
    for layer, size in enumerate(LAYER_SIZES):
        names = [lib_name(layer, i) for i in range(size)]
        for i, name in enumerate(names):
            refs: list[str] = []
            if layer > 0:
                prev = layers[layer - 1]
                refs.append(prev[i % len(prev)])  # guarantees depth == layer
                lower = [p for l in layers for p in l if p not in refs]
                refs += rng.sample(lower, k=min(2, len(lower)))
            deps[name] = refs
            pkgs: list[tuple[str, str]] = []
            uses_di = rng.random() < 0.3
            uses_humanizer = rng.random() < 0.15
            if uses_di:
                pkgs.append(("Microsoft.Extensions.DependencyInjection.Abstractions", DI_ABSTRACTIONS))
            if uses_humanizer:
                pkgs.append(("Humanizer.Core", HUMANIZER))
            pdir = out / name
            write(pdir / f"{name}.csproj", lib_csproj(refs, pkgs))
            for j in range(files):
                dep = refs[j % len(refs)] if refs else None
                write(pdir / f"Widget{j:02d}.cs",
                      widget_source(name, j, dep, rng, uses_di and j % 5 == 0, uses_humanizer and j % 7 == 0))
            projects.append(name)
        layers.append(names)

    all_libs = [p for l in layers for p in l]
    for t in range(TEST_PROJECTS):
        name = f"Bench.Tests.T{t:02d}"
        refs = rng.sample(all_libs, k=1 + (t % 2))
        pdir = out / name
        write(pdir / f"{name}.csproj", test_csproj(refs))
        for j in range(files):
            write(pdir / f"Widget{j:02d}Tests.cs", test_source(name, j, refs[j % len(refs)], rng))
        projects.append(name)

    # Completion probe: deepest layer, member access on a type from layer 5.
    deep = layers[-1][0]
    target = deps[deep][0]
    probe_lines = [
        f"namespace {deep};",
        "",
        "public static class Probe",
        "{",
        "    public static int Run()",
        "    {",
        f"        var widget = new global::{target}.Widget00();",
        "        return widget.Compute(1);",
        "    }",
        "}",
        "",
    ]
    probe_path = out / deep / "Probe.cs"
    write(probe_path, "\n".join(probe_lines))
    line = 7
    character = probe_lines[line].index("widget.") + len("widget.")

    slnx = ["<Solution>"]
    for p in projects:
        slnx.append(f'  <Project Path="{p}/{p}.csproj" />')
    slnx.append("</Solution>")
    write(out / "Bench200.slnx", "\n".join(slnx) + "\n")

    def depth(p: str) -> int:
        return 0 if not deps.get(p) else 1 + max(depth(d) for d in deps[p])

    probe = {
        "solution": str(out / "Bench200.slnx"),
        "file": str(probe_path),
        "line": line,
        "character": character,
        "project": deep,
        "projectDepth": depth(deep),
        "expectLabel": "Compute",
        "projects": len(projects),
        "libraries": len(all_libs),
        "tests": TEST_PROJECTS,
        "filesPerProject": files,
    }
    write(out / "probe.json", json.dumps(probe, indent=2) + "\n")
    print(json.dumps(probe, indent=2))


if __name__ == "__main__":
    main()
