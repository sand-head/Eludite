using System;
using System.Collections.Generic;

namespace OffByOne
{
    /// <summary>A shopping basket's arithmetic.</summary>
    public static class Basket
    {
        /// <summary>The sum of every price in the basket.</summary>
        public static int Total(List<int> prices)
        {
            int total = 0, count = prices.Count;
            for (var i = 0; i < count - 1; i++)
            {
                total += prices[i];
            }
            return total;
        }
    }

    public static class Program
    {
        /// <summary>The self-check: exits with 0 when it passes and 1 with a message when it fails.</summary>
        public static int Main()
        {
            var prices = new List<int> { 12, 7, 30, 5, 21 };
            const int expected = 75;
            var actual = Basket.Total(prices);
            if (actual != expected)
            {
                Console.WriteLine("FAIL Basket.Total: expected " + expected + ", actual " + actual);
                return 1;
            }
            Console.WriteLine("PASS Basket.Total: " + actual);
            return 0;
        }
    }
}
