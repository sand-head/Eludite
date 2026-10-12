using Microsoft.VisualStudio.TestTools.UnitTesting;

namespace Corpus.Artifacts
{
    [TestClass]
    public class ArtifactsTests
    {
        [TestMethod]
        public void Builds()
        {
            Assert.IsTrue(typeof(ArtifactsTests).Assembly.Location.Contains("artifacts"));
        }

        [TestMethod]
        public void Multiplies()
        {
            Assert.AreEqual(6, int.Parse("2") * 3);
        }
    }
}
