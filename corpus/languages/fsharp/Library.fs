/// Eludite corpus: functions over the domain in F# (MIT).
/// let and let rec, curried and tupled parameters, pattern matching, collections, computation expressions and pipelines.
module Eludite.Corpus.Library

open System
open System.IO
open System.Threading.Tasks
open Eludite.Corpus.Domain
open Eludite.Corpus.Domain.Patterns

/// The tax rate, as a literal.
[<Literal>]
let TaxRate = 0.2m

let mutable private counter = 0
let verbose = true

/// A curried function with type annotations.
let addTax (rate: decimal) (amount: decimal<usd>) : decimal<usd> = amount + amount * rate

/// A tupled function.
let describe (name: string, count: int) = sprintf "%s x%d" name count

/// Recursion with an accumulator.
let rec factorial n =
    if n <= 1 then 1 else n * factorial (n - 1)

let rec private fib n acc1 acc2 =
    match n with
    | 0 -> acc1
    | _ -> fib (n - 1) acc2 (acc1 + acc2)

/// Records are copied with `with`.
let rename (customer: Customer) newName =
    { customer with Name = newName; Email = None }

/// Match with guards, active patterns and option.
let discount (order: Order) =
    match order with
    | Discounted rate -> rate
    | Small when order.Customer.Tags.IsEmpty -> 0.0m
    | Small -> 0.02m
    | Large -> 0.05m

let emailOrDefault (customer: Customer) =
    customer.Email |> Option.defaultValue "nobody@example.test"

let firstTag (customer: Customer) =
    match customer.Tags with
    | [] -> None
    | tag :: _ -> Some tag

/// Lists, arrays and sequences.
let squares = [ for i in 1..10 -> i * i ]
let evens = [| 0..2..20 |]
let primes = seq {
    yield 2
    yield! [ 3; 5; 7 ]
    for n in 11..2..30 do
        if Seq.forall (fun d -> n % d <> 0) (seq { 2 .. n / 2 }) then yield n
}

let totals (orders: Order list) =
    orders
    |> List.filter (fun o -> o.Status <> Status.Draft)
    |> List.map (fun o -> (o :> IPriced).Total)
    |> List.sum

let names = List.map (fun (c: Customer) -> c.Name) >> String.concat ", "

/// Async and task computation expressions.
let loadAsync (path: string) = async {
    let! text = File.ReadAllTextAsync path |> Async.AwaitTask
    return text.Split('\n') |> Array.length
}

let saveTask (path: string) (lines: string[]) = task {
    do! File.WriteAllLinesAsync(path, lines)
    counter <- counter + 1
    return counter
}

/// Exceptions.
let tryLoad path =
    try
        let n = loadAsync path |> Async.RunSynchronously
        Ok n
    with
    | :? FileNotFoundException as e -> Error e.Message
    | OrderError(message, id) -> Error $"order {id}: {message}"
    | e -> Error (string e)

let withTiming f =
    let watch = Diagnostics.Stopwatch.StartNew()
    try
        f ()
    finally
        printfn "took %dms" watch.ElapsedMilliseconds

/// Strings: interpolated, verbatim, triple-quoted and a char.
let report (order: Order) =
    let total = (order :> IPriced).Total
    let header = $"Order {order.Id}: {total:N2} for {order.Customer.Name}"
    let path = @"C:\orders\out.txt"
    let template = """
        <order id="{0}">
            {1}
        </order>
    """
    let separator = '-'
    String.Format(template, order.Id, header) + String(separator, 3) + path

let tupleOf (x, y) = x, y
let lambda = fun x y -> x + y
let unit () = ()

#if INTERACTIVE
let interactiveOnly = "running in fsi"
#endif
