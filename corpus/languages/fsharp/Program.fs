/// Eludite corpus: the entry point in F# (MIT).
module Eludite.Corpus.Program

open System
open Eludite.Corpus.Domain
open Eludite.Corpus.Library

[<EntryPoint>]
let main argv =
    let customer = Customer.Create "Ada"
    let order = Order.Empty(customer).Add(Widget(2, 9.99m<usd>)).Add(Gift)
    order.Placed.Add(fun id -> printfn "placed %d" id)
    order.Place()
    printfn "%s" ((order :> IPriced).Describe())
    printfn "discount %M" (discount order)
    let count =
        task {
            let! n = saveTask "out.txt" [| "a"; "b" |]
            return n
        }
        |> fun t -> t.Result
    eprintfn "%d writes, %b" count (argv.Length > 0)
    0
