// Eludite corpus: an F# script (MIT).
#r "nuget: FSharp.Data, 6.4.0"
#r "nuget: Newtonsoft.Json"
#load "Domain.fs"
#load "Library.fs"

open System
open Eludite.Corpus.Domain
open Eludite.Corpus.Library

#if INTERACTIVE
fsi.AddPrinter(fun (c: Customer) -> c.Name)
#endif

let customer = Customer.Create "Grace"
let order = Order(1, customer, [ Bulk(2.5<kg>, 4.0m<usd>) ])

printfn "%s owes %M" customer.Name ((order :> IPriced).Total / 1.0m<usd>)
squares |> List.iter (printfn "%d")
