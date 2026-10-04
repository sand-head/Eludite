using Xunit;

namespace Corpus.XunitV3
{
    public class CalculatorTests
    {
        private readonly ITestOutputHelper _output;

        public CalculatorTests(ITestOutputHelper output)
        {
            _output = output;
        }

        [Fact]
        public void Adds()
        {
            var sum = Calculator.Add(2, 3);
            Assert.Equal(5, sum);
        }

        [Fact]
        public void Subtracts()
        {
            var difference = Calculator.Subtract(2, 3);
            Assert.Equal(1, difference);
        }

        [Fact(Skip = "Division is not written yet")]
        public void Divides()
        {
            Assert.Equal(2, 4 / 2);
        }

        [Fact]
        public void WritesOutput()
        {
            _output.WriteLine("Hello from xunit.v3");
            Assert.True(true);
        }

        [Fact]
        public void Waits()
        {
            // Cancel tests set CORPUS_SLOW_MS so a run is still going when they cancel it.
            var slow = System.Environment.GetEnvironmentVariable("CORPUS_SLOW_MS");
            if (slow != null)
            {
                System.Threading.Thread.Sleep(int.Parse(slow));
            }
        }

        [Theory]
        [InlineData(1, 1, 2)]
        [InlineData(2, 2, 4)]
        [Trait("Category", "Math")]
        public void AddsPairs(int a, int b, int sum)
        {
            Assert.Equal(sum, Calculator.Add(a, b));
        }
    }
}
