namespace Corpus.Arithmetic
{
    // The code Corpus.Coverage and Corpus.Coverlet measure: their tests call Add (never with b == 0, so its if covers one
    // branch of two) and Subtract; nothing calls Divide. A library of its own, since both collectors leave the test
    // assembly out by default.
    public class Calculator
    {
        public int Add(int a, int b)
        {
            if (b == 0)
            {
                return a;
            }
            else
            {
                return a + b;
            }
        }

        public int Subtract(int a, int b)
        {
            return a - b;
        }

        public int Divide(int a, int b)
        {
            return a / b;
        }
    }
}
