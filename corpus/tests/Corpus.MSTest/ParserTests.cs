using Microsoft.VisualStudio.TestTools.UnitTesting;

namespace Corpus.MSTest
{
    [TestClass]
    public class ParserTests
    {
        public TestContext TestContext { get; set; }

        [TestMethod]
        public void ParsesNumbers()
        {
            var value = int.Parse("42");
            Assert.AreEqual(42, value);
        }

        [TestMethod]
        public void ParsesNegatives()
        {
            var value = int.Parse("-7");
            Assert.AreEqual(7, value);
        }

        [TestMethod]
        [Ignore("Hexadecimal comes later")]
        public void ParsesHex()
        {
            Assert.Fail("not run");
        }

        [TestMethod]
        public void WritesOutput()
        {
            TestContext.WriteLine("Hello from MSTest");
            System.Console.WriteLine("Console from MSTest");
        }

        [TestMethod]
        [DataRow("1", 1)]
        [DataRow("2", 2)]
        [TestCategory("Rows")]
        public void ParsesRows(string text, int expected)
        {
            Assert.AreEqual(expected, int.Parse(text));
        }
    }
}
