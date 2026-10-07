/// Eludite corpus: a signature file for the domain (MIT).
/// Records, unions, enums, exceptions, interfaces and module values; no member signatures (see Members.fsi).
namespace Eludite.Corpus.Domain

open System

(* Units of measure. *)
[<Measure>] type kg
[<Measure>] type usd

/// A customer, as a record.
[<CustomEquality; NoComparison>]
type Customer =
    { Id: Guid
      Name: string
      Email: string option
      Tags: string list }

/// What was ordered.
type Item =
    | Widget of count: int * unitPrice: decimal<usd>
    | Bulk of weight: float<kg> * pricePerKilo: decimal<usd>
    | Gift

type Status =
    | Draft = 0
    | Placed = 1
    | Shipped = 2

/// Raised when an order cannot be placed.
exception OrderError of message: string * orderId: int

/// Anything that can be priced.
type IPriced =
    abstract Total: decimal<usd>
    abstract Describe: unit -> string

/// Active patterns over an order's size.
module Patterns =
    val (|Small|Large|) : order: Order -> Choice<unit, unit>
    val (|Discounted|_|) : order: Order -> decimal option
    val free: IPriced
    val price: rate: decimal -> amount: decimal<usd> -> decimal<usd>
