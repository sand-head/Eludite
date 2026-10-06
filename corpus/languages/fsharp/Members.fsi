/// Eludite corpus (MIT): member signatures in a signature file, which the signature grammar does not parse:
/// a class with constructors, members, a property with get and set and an interface, and members on a record.
namespace Eludite.Corpus.Domain

type Order =
    new: id: int * customer: Customer * items: Item list -> Order
    member Id: int
    member Status: Status with get, set
    member Place: unit -> unit
    static member Empty: customer: Customer -> Order
    interface IPriced

type Customer with
    override Equals: other: obj -> bool
    static member Create: name: string -> Customer
