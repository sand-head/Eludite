using System.Collections.Generic;
using NUnit.Framework;

namespace Corpus.NUnit
{
    [TestFixture]
    public class StackTests
    {
        [Test]
        public void Pushes()
        {
            var stack = new Stack<int>();
            stack.Push(1);
            Assert.That(stack.Count, Is.EqualTo(1));
        }

        [Test]
        public void Pops()
        {
            var stack = new Stack<int>();
            stack.Push(1);
            stack.Pop();
            Assert.That(stack.Count, Is.EqualTo(1));
        }

        [Test]
        [Ignore("Peeking is not decided yet")]
        public void Peeks()
        {
            Assert.Fail("not run");
        }

        [Test]
        public void WritesOutput()
        {
            TestContext.Out.WriteLine("Hello from NUnit");
            Assert.Pass();
        }

        [TestCase(1, 2)]
        [TestCase(3, 4)]
        [Category("Pairs")]
        public void PushesPairs(int a, int b)
        {
            var stack = new Stack<int>();
            stack.Push(a);
            stack.Push(b);
            Assert.That(stack.Pop(), Is.EqualTo(b));
        }
    }
}
