# corpus/languages/

Hand-written Visual Basic and F# fixtures, MIT (brief 0057), shaped like real code and exercising the constructs the
brief lists. Used by `crates/editor/src/syntax/dotnet_tests.rs`: the highlighting tests read positions from them, and
`every_fixture_parses_cleanly_or_with_its_known_error_count` parses each with its grammar (`tree-sitter-vb-dotnet`
0.1.0, `tree-sitter-fsharp` 0.3.12) and asserts no `ERROR` or `MISSING` node, except for the files below that hold one
construct the grammar does not support, each with the error-node count it produces today, so a grammar bump that
changes it is noticed. The gaps are written up in `docs/briefs/0057-report.md`.

## Visual Basic (`vb/`)

Clean (no error node):

| File | What it exercises |
|---|---|
| `Program.vb` | `Option Strict`/`Explicit`/`Infer`, `Imports`, `Namespace`, a `Module` with `Sub Main`, `'''` doc comments, `Const`, a `Date` literal, type characters (`%`, `@`, `!`), hex and octal literals, a `Char` literal, doubled quotes, an interpolated string with a format specifier, `If`/`ElseIf`/`Else`, `For ... Step`, `For Each`, `Do While`, `Do ... Loop Until`, `While` with `Exit While`, `Continue For`, `ReDim Preserve`, `Select Case` with ranges, `Is >` and `Case Else`, single-line and multi-line `Function` and `Sub` lambdas, `With`, `Try`/`Catch ... When`/`Catch`/`Finally`, `Throw`, `Using`, `SyncLock`, line continuations, `GoTo` and a label, `Like`, `Not`/`AndAlso`/`OrElse`/`Xor`, `Optional` and `ParamArray` parameters, `ByVal`/`ByRef`, `#If` inside a procedure, `Nothing`, `True`/`False`. |
| `Shapes.vb` | An `Interface`, `Enum`s (one with explicit values and `<Flags>`), `Delegate Function` and `Delegate Sub`, a `Structure` with `Sub New` and `Overrides`, a `MustInherit Class Shape Implements IShape` with `Shared`, `Protected`, `MustOverride`, `Overridable`, `Overloads`, auto and full properties (`Get`/`Set`), a `NotInheritable Class Circle Inherits Shape` with `MyBase.New`, a property attribute, `NameOf`, `Default` indexed property, `With { .X = ... }` object initializers, `CType`. |

One unsupported construct per file (the count is the number of `ERROR` and `MISSING` nodes today):

| File | Construct | Count |
|---|---|---|
| `Header.vb` | A comment line before `Option`, a blank line between `Option` and `Imports`: the grammar allows nothing between or before them | 2 |
| `Generics.vb` | Generic types and declarations: `Dictionary(Of String, T)`, `Class Registry(Of T As {...})`, `Function F(Of T)(...)` (the grammar's `Of` lists have no parentheses) | 9 |
| `AsNew.vb` | `As New T(...)` in a field, a `Dim` and a `Using` | 5 |
| `InheritsLine.vb` | `Inherits` and `Implements` on their own lines after the `Class` line | 2 |
| `RaiseEvent.vb` | `Event` declarations with the `RaiseEvent` statement | 2 |
| `AddHandler.vb` | The `AddHandler` and `RemoveHandler` statements with `AddressOf` and a lambda | 5 |
| `Handles.vb` | `WithEvents` and methods with a `Handles` clause | 2 |
| `CustomEvent.vb` | `Custom Event` with `AddHandler`, `RemoveHandler` and `RaiseEvent` accessors | 12 |
| `AsyncAwait.vb` | `Await` in `Async Function` and `Async Sub` (`Async` itself is a known modifier) | 3 |
| `Iterator.vb` | `Yield` in an `Iterator Function` | 2 |
| `Linq.vb` | A LINQ query (`From ... Where ... Order By ... Select`) | 4 |
| `XmlLiteral.vb` | An XML literal with `<%= %>` | 9 |
| `Regions.vb` | `#Const`, `#Region` and `#If` between declarations (directives parse only inside a procedure) | 8 |
| `Operators.vb` | `Operator +` and a `Widening Operator CType` | 9 |
| `ImplementsMember.vb` | `Implements I.Member` on a property and a method | 2 |
| `EnumBase.vb` | `Enum E As Byte` | 1 |
| `ArrayDeclarations.vb` | `Dim arr() As Integer = {...}` and `Dim arr(2) As String` | 3 |
| `ForEachTyped.vb` | `For Each x As T In xs` | 1 |
| `WithBlock.vb` | A `With` block whose statements start with `.Member` | 3 |
| `TypeOfIs.vb` | `TypeOf x Is T` | 1 |
| `IfCoalesce.vb` | The two-argument `If(a, b)` | 1 |
| `NullConditional.vb` | `x?.Member` | 2 |
| `CollectionInitializer.vb` | `New T From {...}` | 1 |
| `AttributeInline.vb` | An attribute on the same line as its declaration (`<Serializable> Public Class`) | 2 |
| `KeywordPrefixes.vb` | Identifiers starting with a keyword where that keyword is valid (`Document.Save()`, `Format(x)`, `Subtotal`, `Notify()`, `Private Subscriptions`): the keyword token takes lexical precedence over the longer identifier | 5 |

## F# (`fsharp/`)

Clean (no error node):

| File | What it exercises |
|---|---|
| `Domain.fs` | `namespace`, `open`, `(* *)` and `///` comments, `[<Measure>]` units, a record with attributes, `override` members, `static member`, a union with named tuple fields and a member, an enum, `exception ... of`, an interface, a class with a primary constructor, `let mutable`, a secondary `new`, properties with `get`/`set`, `member val`, `[<CLIEvent>]`, `raise`, `interface ... with`, active patterns (`(|Small|Large|)`, `(|Discounted|_|)`), an object expression, `Dictionary<string, int>()`. |
| `Library.fs` | A `module`, `[<Literal>]`, `let mutable private`, curried and tupled functions with type annotations, `let rec`, `{ x with }`, `match` with guards, active patterns and `option`, list comprehensions, array ranges, `seq { }` with `yield`/`yield!`, pipelines and `>>`, lambdas, `async { }` with `let!`, `task { }` with `do!`, `try ... with` (`:?`, exception patterns), `try ... finally`, `Ok`/`Error`, an interpolated string with a format specifier, a verbatim string, a triple-quoted string, a char, `#if INTERACTIVE`. |
| `Program.fs` | `[<EntryPoint>]`, method chains, an event subscription, `task { }` and `printfn`/`eprintfn`. |
| `Script.fsx` | `#r "nuget: ..."`, `#load`, `fsi.AddPrinter` under `#if INTERACTIVE`, measures in literals. |
| `Domain.fsi` | A signature file: `namespace`, `open`, measures, a record, a union, an enum, an exception, an interface with `abstract` members, and a `module` with `val` signatures including active patterns. |

One unsupported construct per file:

| File | Construct | Count |
|---|---|---|
| `Members.fsi` | Member signatures in a signature file (`new:`, `member`, `static member`, `override`, `with get, set`, `interface` inside a type, a type extension): the signature grammar has only `abstract` members and `val` | 3 |
