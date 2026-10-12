using Corpus.Arithmetic;
using Microsoft.VisualStudio.TestTools.UnitTesting;

namespace Corpus.Coverage
{
    [TestClass]
    public class CalculatorTests
    {
        [TestMethod]
        public void Adds()
        {
            Assert.AreEqual(5, new Calculator().Add(2, 3));
        }

        [TestMethod]
        public void Subtracts()
        {
            Assert.AreEqual(1, new Calculator().Subtract(3, 2));
        }
    }
}
