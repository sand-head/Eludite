using Microsoft.VisualStudio.TestTools.UnitTesting;

namespace Corpus.PropsDriven
{
    [TestClass]
    public class PropsTests
    {
        [TestMethod]
        public void ReadsProps()
        {
            Assert.AreEqual(4, int.Parse("4"));
        }

        [TestMethod]
        public void FailsFromProps()
        {
            Assert.AreEqual(5, int.Parse("4"), "Four is not five");
        }
    }
}
