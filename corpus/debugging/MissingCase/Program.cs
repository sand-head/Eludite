using System;

namespace MissingCase
{
    public enum Coin
    {
        Penny,
        Nickel,
        Dime,
        Quarter,
    }

    /// <summary>What coins are worth.</summary>
    public static class Coins
    {
        /// <summary>The value of one coin, in cents.</summary>
        public static int Cents(Coin coin)
        {
            switch (coin)
            {
                case Coin.Penny:
                    return 1;
                case Coin.Nickel:
                    return 5;
                case Coin.Dime:
                    return 10;
                default:
                    return 0;
            }
        }
    }

    public static class Program
    {
        /// <summary>The self-check: exits with 0 when it passes and 1 with a message when it fails.</summary>
        public static int Main()
        {
            var purse = new[] { Coin.Quarter, Coin.Dime, Coin.Nickel, Coin.Penny };
            var expected = new[] { 25, 10, 5, 1 };
            for (var i = 0; i < purse.Length; i++)
            {
                var actual = Coins.Cents(purse[i]);
                if (actual != expected[i])
                {
                    Console.WriteLine("FAIL Coins.Cents(" + purse[i] + "): expected " + expected[i] + ", actual " + actual);
                    return 1;
                }
            }
            Console.WriteLine("PASS Coins.Cents");
            return 0;
        }
    }
}
