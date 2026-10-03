using Eludite.Debugger.Mono;

namespace Eludite.Debugger.Mono.Tests;

/// <summary>
/// Brief 0036: the source scanner behind the adapter's type-name resolution finds the namespaces, <c>using</c>
/// directives and types around a line, whatever C# the file is written in.
/// </summary>
public sealed class SourceScopesTests
{
    private static int LineOf(string text, string marker) =>
        text.Split('\n').Select((l, i) => (l, i)).First(x => x.l.Contains(marker, StringComparison.Ordinal)).i + 1;

    [Fact]
    public void A_block_namespace_with_top_level_usings()
    {
        const string text = """
            using System;
            using IO = System.IO;
            using static System.Math;

            namespace MissingCase
            {
                public enum Coin { Penny, Quarter }

                public static class Coins
                {
                    public static int Cents(Coin coin)
                    {
                        return 0; // here
                    }
                }
            }
            """;
        var scopes = SourceScopes.Parse(text);
        var (types, levels) = scopes.At(LineOf(text, "// here"));
        Assert.Equal(["MissingCase.Coins"], types);
        Assert.Equal(["MissingCase", string.Empty], levels.Select(l => l.Namespace));
        Assert.Empty(levels[0].Imports.Namespaces);
        Assert.Equal(["System"], levels[1].Imports.Namespaces);
        Assert.Equal("System.IO", levels[1].Imports.Aliases["IO"]);
        Assert.Equal("MissingCase", scopes.NamespaceAt(LineOf(text, "// here")));
        // Outside the namespace only the file's level is left.
        Assert.Equal([string.Empty], scopes.At(1).Levels.Select(l => l.Namespace));
    }

    [Fact]
    public void Nested_namespaces_keep_their_usings_at_their_level_and_statements_are_not_directives()
    {
        const string text = """
            global using System.Text;
            namespace A.B
            {
                using A.Colors;
                namespace C
                {
                    using Shapes = A.Shapes;
                    class Outer
                    {
                        class Inner
                        {
                            void M()
                            {
                                using (var s = new System.IO.MemoryStream()) { } // here
                                using var t = new System.IO.MemoryStream();
                            }
                        }
                    }
                }
            }
            """;
        var scopes = SourceScopes.Parse(text);
        var (types, levels) = scopes.At(LineOf(text, "// here"));
        Assert.Equal(["A.B.C.Outer.Inner", "A.B.C.Outer"], types);
        Assert.Equal(["A.B.C", "A.B", "A", string.Empty], levels.Select(l => l.Namespace));
        Assert.Equal("A.Shapes", levels[0].Imports.Aliases["Shapes"]);
        Assert.Equal(["A.Colors"], levels[1].Imports.Namespaces);
        Assert.Empty(levels[2].Imports.Namespaces);
        Assert.Equal(["System.Text"], levels[3].Imports.Namespaces);
    }

    [Fact]
    public void A_file_scoped_namespace_records_and_strings_with_braces()
    {
        const string text = """"
            using System.Collections.Generic;
            namespace Shop.Orders;

            public record Line(string Sku, int Count);

            public sealed class Basket
            {
                private const string Brace = "{ not a block // nor a comment";
                private const string Verbatim = @"a ""quoted"" } brace";
                private const char Open = '{';
                private readonly string _raw = """
                    { "json": [ } ]
                    """;

                public string Describe(int n) => $"{n} items {(n > 1 ? "}" : "{")} {{literal}}";

                public int Total() // here
                {
                    return 0;
                }
            }

            public struct After
            {
                public int X; // after
            }
            """";
        var scopes = SourceScopes.Parse(text);
        var (types, levels) = scopes.At(LineOf(text, "// here"));
        Assert.Equal(["Shop.Orders.Basket"], types);
        Assert.Equal(["Shop.Orders", "Shop", string.Empty], levels.Select(l => l.Namespace));
        Assert.Equal(["System.Collections.Generic"], levels[^1].Imports.Namespaces);
        // The braces in the strings and the character did not unbalance the class: the next type is found.
        Assert.Equal(["Shop.Orders.After"], scopes.At(LineOf(text, "// after")).Types);
    }
}
