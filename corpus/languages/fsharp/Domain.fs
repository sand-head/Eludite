/// Eludite corpus: the domain types of a small order system in F# (MIT).
/// Records, unions, units of measure, classes, interfaces, object expressions, exceptions and attributes.
namespace Eludite.Corpus.Domain

open System
open System.Collections.Generic

(* Units of measure keep quantities apart at compile time. *)
[<Measure>] type kg
[<Measure>] type usd

/// A customer, as a record.
[<CustomEquality; NoComparison>]
type Customer =
    { Id: Guid
      Name: string
      Email: string option
      Tags: string list }

    override this.Equals(other) =
        match other with
        | :? Customer as c -> this.Id = c.Id
        | _ -> false

    override this.GetHashCode() = this.Id.GetHashCode()

    /// A customer with no tags.
    static member Create(name: string) =
        { Id = Guid.NewGuid(); Name = name; Email = None; Tags = [] }

/// What was ordered: a discriminated union with record and tuple cases.
type Item =
    | Widget of count: int * unitPrice: decimal<usd>
    | Bulk of weight: float<kg> * pricePerKilo: decimal<usd>
    | Gift

    member this.Price =
        match this with
        | Widget(count, unitPrice) -> decimal count * unitPrice
        | Bulk(weight, pricePerKilo) -> decimal (float weight) * pricePerKilo
        | Gift -> 0m<usd>

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

/// An order: a class with a primary constructor, mutable state, properties and members.
type Order(id: int, customer: Customer, items: Item list) =
    let mutable status = Status.Draft
    let placed = Event<int>()

    new(customer) = Order(0, customer, [])

    member _.Id = id
    member _.Customer = customer
    member _.Items = items

    member _.Status
        with get () = status
        and set value = status <- value

    member val Note = "" with get, set

    [<CLIEvent>]
    member _.Placed = placed.Publish

    member this.Place() =
        if List.isEmpty items then raise (OrderError("no items", id))
        this.Status <- Status.Placed
        placed.Trigger id

    member this.Add(item: Item) = Order(id, customer, item :: items)

    interface IPriced with
        member _.Total = items |> List.sumBy (fun i -> i.Price)
        member this.Describe() = $"Order {id} for {customer.Name}: {items.Length} items"

    static member Empty(customer) = Order(customer)

/// Active patterns over an order's size.
module Patterns =
    let (|Small|Large|) (order: Order) =
        if order.Items.Length < 10 then Small else Large

    let (|Discounted|_|) (order: Order) =
        if order.Customer.Tags |> List.contains "vip" then Some 0.1m else None

    /// An object expression implementing IPriced on the fly.
    let free =
        { new IPriced with
            member _.Total = 0m<usd>
            member _.Describe() = "free" }

    let inventory = Dictionary<string, int>()
