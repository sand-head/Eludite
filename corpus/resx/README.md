# corpus/resx

Resource files, their designer files and the projects that list them, for proposal 0005's `.resx` editor (MIT,
written for the tests). The tests copy what they edit to a temporary directory; nothing here is built.

| Entry | What it exercises |
|---|---|
| `Strings/Properties/Resources.resx` | Visual Studio's bytes (a byte order mark, CRLF, no final newline, two-space indentation): a key with a placeholder and a comment, an invariant key (`{Invariant}` in the comment), a value with a line break, one with `&`, `<` and `>`, a `ResXFileRef` icon (not a string), and the keys `1Number`, `with space` and `class`, which the designer renames `_1Number`, `with_space` and `_class` |
| `Strings/Properties/Resources.de.resx` | A German file missing `Brand` (invariant), `Ampersand`, `1Number`, `with space` and `class`, with `Orphan`, a key the neutral file does not have |
| `Strings/Properties/Resources.fr-FR.resx` | LF line endings, no byte order mark, a final newline; `Hello` with the wrong placeholder (`{1}`), `Save` untranslated, `Brand` translated although invariant |
| `Strings/Properties/Resources.Designer.cs` | What `ResXFileCodeGenerator` writes for the neutral file (`internal`, namespace `Corpus.Strings.Properties`, members sorted ignoring case, `&amp;` in a summary, the two-line summary of `Multi`, the `System.Drawing.Icon` property), CRLF |
| `Strings/Strings.csproj` | An SDK project: `RootNamespace` and `NeutralLanguage`, the `Update` items with `Generator`, `LastGenOutput` and the designer's `DependentUpon` |
| `Public/Messages.resx`, `Messages.es.resx`, `Messages.Designer.cs`, `Public.csproj` | `PublicResXFileCodeGenerator` with a `CustomToolNamespace` (`Corpus.Shared`) and the files in the project folder |
| `Legacy/Legacy.csproj` | A legacy (non-SDK) project, CRLF, with explicit `EmbeddedResource` items: the generator metadata, a culture file with `DependentUpon`, a WinForms form's file and `Default.aspx.resx` |
| `Legacy/Form1.resx` | A WinForms form's file: `metadata`, `$this.Icon` (base64) and `>>` entries, no string |
| `Legacy/Properties/Resources.resx`, `Resources.de.resx`, `Resources.Designer.cs` | LF, no byte order mark; `Title` untranslated in German; the designer in `Corpus.Legacy.Properties` |
| `Legacy/Default.aspx.resx` | A neutral file whose base name has a dot (`aspx` is not a culture) |
| `Corpus.slnx`, `Legacy.slnx` | The two SDK projects; the legacy project alone |

Expected outputs are in the tests: `crates/resx/tests/corpus.rs` (every file parses, round-trips byte for byte
through every kind of splice, and the sets, rows and warnings are as listed above) and
`dotnet/tests/Eludite.Host.Tests/Resx*.cs` (`eludite/resx/sets` lists these files with their metadata; regenerating
every designer file reproduces its bytes).
