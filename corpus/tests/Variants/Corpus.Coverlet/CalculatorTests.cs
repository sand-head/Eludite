using Corpus.Arithmetic;
using Xunit;

namespace Corpus.Coverlet
{
    public class CalculatorTests
    {
        [Fact]
        public void Adds()
        {
            Assert.Equal(5, new Calculator().Add(2, 3));
        }

        [Fact]
        public void Subtracts()
        {
            Assert.Equal(1, new Calculator().Subtract(3, 2));
        }
    }
}
