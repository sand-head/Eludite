using System.Threading;

namespace Eludite.Debugger.Mono.TestApp
{
    /// <summary>An object with fields and properties for the Locals window's expansion.</summary>
    public sealed class Order
    {
        private readonly int _id;

        public Order(int id, string customer)
        {
            _id = id;
            Customer = customer;
            Tags = new[] { "new", "paid" };
        }

        public int Id => _id;

        public string Customer { get; set; }

        public string[] Tags { get; }

        public int Total;

        public string Describe() => Customer + " #" + _id;
    }

    /// <summary>A method with <c>this</c>, parameters and locals, and a callee to step into.</summary>
    public sealed class Calculator
    {
        private int _calls;

        public string Name { get; } = "calc";

        public int Calls => _calls;

        public int Add(int a, int b)
        {
            _calls++;
            var sum = a + b; // MARK: add-sum
            var doubled = Twice(sum); // MARK: add-twice
            return doubled; // MARK: add-return
        }

        public static int Twice(int x)
        {
            var r = x * 2; // MARK: twice
            return r;
        }
    }

    public static class Program
    {
        public static int Main(string[] args)
        {
            var mode = args.Length > 0 ? args[0] : "run";
            if (mode == "sleep")
            {
                System.Console.WriteLine("sleeping"); // MARK: sleep-print
                Thread.Sleep(60000); // MARK: sleep
                return 0;
            }

            var names = new string[25];
            for (var n = 0; n < names.Length; n++)
            {
                names[n] = "name" + n;
            }

            var big = new int[1000];
            var order = new Order(7, "Contoso");
            var calc = new Calculator();
            var result = calc.Add(2, 3); // MARK: main-add
            System.Console.WriteLine("result " + result);
            var total = 0;
            for (var i = 0; i < 100; i++)
            {
                total += i; // MARK: loop-body
            }

            try
            {
                Fail(); // MARK: call-fail
            }
            catch (System.InvalidOperationException e)
            {
                System.Console.Error.WriteLine("caught " + e.Message);
            }

            Many();
            System.Console.WriteLine("done " + total + " " + order.Describe() + " " + names.Length + " " + big.Length); // MARK: done
            return 3;
        }

        public static void Fail()
        {
            throw new System.InvalidOperationException("boom"); // MARK: throw
        }

        /// <summary>Never returns: an evaluation of it must time out.</summary>
        public static int Hang()
        {
            Thread.Sleep(Timeout.Infinite);
            return 0;
        }

        /// <summary>A frame with 200 locals (the budget of stackTrace, scopes and variables).</summary>
        public static int Many()
        {
#pragma warning disable CS0219 // Locals for the debugger to read.
            var l000 = 0;
            var l001 = 1;
            var l002 = 2;
            var l003 = 3;
            var l004 = 4;
            var l005 = 5;
            var l006 = 6;
            var l007 = 7;
            var l008 = 8;
            var l009 = 9;
            var l010 = 10;
            var l011 = 11;
            var l012 = 12;
            var l013 = 13;
            var l014 = 14;
            var l015 = 15;
            var l016 = 16;
            var l017 = 17;
            var l018 = 18;
            var l019 = 19;
            var l020 = 20;
            var l021 = 21;
            var l022 = 22;
            var l023 = 23;
            var l024 = 24;
            var l025 = 25;
            var l026 = 26;
            var l027 = 27;
            var l028 = 28;
            var l029 = 29;
            var l030 = 30;
            var l031 = 31;
            var l032 = 32;
            var l033 = 33;
            var l034 = 34;
            var l035 = 35;
            var l036 = 36;
            var l037 = 37;
            var l038 = 38;
            var l039 = 39;
            var l040 = 40;
            var l041 = 41;
            var l042 = 42;
            var l043 = 43;
            var l044 = 44;
            var l045 = 45;
            var l046 = 46;
            var l047 = 47;
            var l048 = 48;
            var l049 = 49;
            var l050 = 50;
            var l051 = 51;
            var l052 = 52;
            var l053 = 53;
            var l054 = 54;
            var l055 = 55;
            var l056 = 56;
            var l057 = 57;
            var l058 = 58;
            var l059 = 59;
            var l060 = 60;
            var l061 = 61;
            var l062 = 62;
            var l063 = 63;
            var l064 = 64;
            var l065 = 65;
            var l066 = 66;
            var l067 = 67;
            var l068 = 68;
            var l069 = 69;
            var l070 = 70;
            var l071 = 71;
            var l072 = 72;
            var l073 = 73;
            var l074 = 74;
            var l075 = 75;
            var l076 = 76;
            var l077 = 77;
            var l078 = 78;
            var l079 = 79;
            var l080 = 80;
            var l081 = 81;
            var l082 = 82;
            var l083 = 83;
            var l084 = 84;
            var l085 = 85;
            var l086 = 86;
            var l087 = 87;
            var l088 = 88;
            var l089 = 89;
            var l090 = 90;
            var l091 = 91;
            var l092 = 92;
            var l093 = 93;
            var l094 = 94;
            var l095 = 95;
            var l096 = 96;
            var l097 = 97;
            var l098 = 98;
            var l099 = 99;
            var l100 = 100;
            var l101 = 101;
            var l102 = 102;
            var l103 = 103;
            var l104 = 104;
            var l105 = 105;
            var l106 = 106;
            var l107 = 107;
            var l108 = 108;
            var l109 = 109;
            var l110 = 110;
            var l111 = 111;
            var l112 = 112;
            var l113 = 113;
            var l114 = 114;
            var l115 = 115;
            var l116 = 116;
            var l117 = 117;
            var l118 = 118;
            var l119 = 119;
            var l120 = 120;
            var l121 = 121;
            var l122 = 122;
            var l123 = 123;
            var l124 = 124;
            var l125 = 125;
            var l126 = 126;
            var l127 = 127;
            var l128 = 128;
            var l129 = 129;
            var l130 = 130;
            var l131 = 131;
            var l132 = 132;
            var l133 = 133;
            var l134 = 134;
            var l135 = 135;
            var l136 = 136;
            var l137 = 137;
            var l138 = 138;
            var l139 = 139;
            var l140 = 140;
            var l141 = 141;
            var l142 = 142;
            var l143 = 143;
            var l144 = 144;
            var l145 = 145;
            var l146 = 146;
            var l147 = 147;
            var l148 = 148;
            var l149 = 149;
            var l150 = 150;
            var l151 = 151;
            var l152 = 152;
            var l153 = 153;
            var l154 = 154;
            var l155 = 155;
            var l156 = 156;
            var l157 = 157;
            var l158 = 158;
            var l159 = 159;
            var l160 = 160;
            var l161 = 161;
            var l162 = 162;
            var l163 = 163;
            var l164 = 164;
            var l165 = 165;
            var l166 = 166;
            var l167 = 167;
            var l168 = 168;
            var l169 = 169;
            var l170 = 170;
            var l171 = 171;
            var l172 = 172;
            var l173 = 173;
            var l174 = 174;
            var l175 = 175;
            var l176 = 176;
            var l177 = 177;
            var l178 = 178;
            var l179 = 179;
            var l180 = 180;
            var l181 = 181;
            var l182 = 182;
            var l183 = 183;
            var l184 = 184;
            var l185 = 185;
            var l186 = 186;
            var l187 = 187;
            var l188 = 188;
            var l189 = 189;
            var l190 = 190;
            var l191 = 191;
            var l192 = 192;
            var l193 = 193;
            var l194 = 194;
            var l195 = 195;
            var l196 = 196;
            var l197 = 197;
            var l198 = 198;
            var l199 = 199;
#pragma warning restore CS0219
            var last = l000 + l199; // MARK: many
            return last;
        }
    }
}
