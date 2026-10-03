using Xunit;
using Xunit.Abstractions;

namespace Corpus.Xunit2
{
    public class GreeterTests
    {
        private readonly ITestOutputHelper _output;

        public GreeterTests(ITestOutputHelper output)
        {
            _output = output;
        }

        [Fact]
        public void Greets()
        {
            var text = Greeter.Greet("Ada");
            Assert.Equal("Hello, Ada!", text);
        }

        [Fact]
        public void GreetsNobody()
        {
            var text = Greeter.Greet("");
            Assert.Equal("Hello, stranger!", text);
        }

        [Fact(Skip = "Farewells come later")]
        public void SaysGoodbye()
        {
            Assert.True(false);
        }

        [Fact]
        public void WritesOutput()
        {
            _output.WriteLine("Hello from xunit 2");
            Assert.True(true);
        }

        [Fact]
        [Trait("Category", "Slow")]
        public void Waits()
        {
            // Cancel tests set CORPUS_SLOW_MS so a run is still going when they cancel it.
            var slow = System.Environment.GetEnvironmentVariable("CORPUS_SLOW_MS");
            if (slow != null)
            {
                System.Threading.Thread.Sleep(int.Parse(slow));
            }
        }
    }

    public static class Greeter
    {
        public static string Greet(string name)
        {
            return "Hello, " + name + "!";
        }
    }
}
