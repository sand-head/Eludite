using System;
using System.Collections.Generic;

namespace NullField
{
    /// <summary>A folder in a tree: its name, its parent (null for the root) and its path from the root.</summary>
    public sealed class Folder
    {
        public readonly string Name;
        public readonly Folder Parent;
        public string Path;

        public Folder(string name, Folder parent)
        {
            Name = name;
            if (parent != null)
            {
                Parent = parent;
                Path = parent.Path + "/" + name;
            }
        }
    }

    /// <summary>Reports over a folder tree.</summary>
    public static class Tree
    {
        /// <summary>The lengths of the paths from <paramref name="folder"/> up to the root, joined with commas.</summary>
        public static string Describe(Folder folder)
        {
            var lengths = new List<string>();
            for (var f = folder; f != null; f = f.Parent)
            {
                lengths.Add(f.Path.Length.ToString());
            }
            return string.Join(",", lengths);
        }
    }

    public static class Program
    {
        private const string Check = "Tree.Describe";
        private const string Expected = "21,10,5";

        /// <summary>The self-check: exits with 0 when it passes and 1 with a message when it fails.</summary>
        public static int Main()
        {
            AppDomain.CurrentDomain.UnhandledException += (sender, e) =>
            {
                var error = (Exception)e.ExceptionObject;
                Console.WriteLine("FAIL " + Check + ": expected " + Expected + ", actual " + error.GetType().Name + " (" + error.Message + ")");
                Console.Out.Flush();
                Environment.Exit(1);
            };
            var root = new Folder("root", null);
            var docs = new Folder("docs", root);
            var report = new Folder("report.txt", docs);
            var actual = Tree.Describe(report);
            if (actual != Expected)
            {
                Console.WriteLine("FAIL " + Check + ": expected " + Expected + ", actual " + actual);
                return 1;
            }
            Console.WriteLine("PASS " + Check + ": " + actual);
            return 0;
        }
    }
}
