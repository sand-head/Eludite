namespace Corpus.FSharp

open System
open System.Collections.Generic
open NUnit.Framework

[<TestFixture>]
type QueueTests() =

    [<Test>]
    member _.Enqueues() =
        let queue = Queue<int>()
        queue.Enqueue 1
        Assert.That(queue.Count, Is.EqualTo 1)

    [<Test>]
    member _.Dequeues() =
        let queue = Queue<int>()
        queue.Enqueue 1
        queue.Dequeue() |> ignore
        Assert.That(queue.Count, Is.EqualTo 1, "the queue is empty after its one item is taken")

    [<Test>]
    [<Ignore("Peeking is not decided yet")>]
    member _.Peeks() = Assert.Fail "not run"

    [<Test>]
    member _.WritesOutput() =
        TestContext.Out.WriteLine "Hello from F#"
        Console.WriteLine "Console from F#"
        Assert.Pass()

    [<Test>]
    member _.KeepsOrder() =
        let queue = Queue<string>()
        queue.Enqueue "first"
        queue.Enqueue "second"
        Assert.That(queue.Dequeue(), Is.EqualTo "first")
