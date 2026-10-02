// The brief 0004 debuggee. It prints "ready <pid>" once the CLR is up, then calls Tick in a loop with a pause
// between iterations, printing "tick <i> <QueryPerformanceCounter>" just before each call (the test's clock for the
// breakpoint-hit-to-stopped budget). The end-to-end test sets its breakpoint on the line marked BREAKPOINT and reads
// `counter`.
using System;
using System.Diagnostics;
using System.Threading;

namespace Eludite.Fixtures.Counter
{
    internal static class Program
    {
        private static int Main(string[] args)
        {
            int delayMs = args.Length > 0 ? int.Parse(args[0]) : 50;
            Console.WriteLine("ready " + Process.GetCurrentProcess().Id);
            Console.Out.Flush();
            int total = 0;
            for (int i = 0; ; i++)
            {
                Console.WriteLine("tick " + i + " " + Stopwatch.GetTimestamp());
                total += Tick(i);
                Thread.Sleep(delayMs);
            }
        }

        private static int Tick(int iteration)
        {
            int counter = iteration * 2;
            counter = counter + 1; // BREAKPOINT
            return counter;
        }
    }
}
