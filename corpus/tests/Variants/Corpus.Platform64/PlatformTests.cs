using System;
using Xunit;

namespace Corpus.Platform64
{
    public class PlatformTests
    {
        [Fact]
        public void RunsIn64Bits()
        {
            Assert.Equal(8, IntPtr.Size);
        }

        [Fact]
        public void Adds()
        {
            Assert.Equal(3, 1 + 2);
        }
    }
}
