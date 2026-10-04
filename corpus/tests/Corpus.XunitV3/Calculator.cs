namespace Corpus.XunitV3
{
    /// <summary>What the tests test. Its own file, so its CodeLens counts references from another file (brief 0052).</summary>
    public static class Calculator
    {
        public static int Add(int a, int b)
        {
            return a + b;
        }

        public static int Subtract(int a, int b)
        {
            return a - b;
        }
    }
}
