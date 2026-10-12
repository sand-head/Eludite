using System;
using System.Collections.Generic;
using System.Linq;

namespace Corpus.Outlining
{
    /// <summary>
    /// A class with regions, members and blocks to fold (brief 0067).
    /// </summary>
    public class Inventory
    {
        #region Fields
        private readonly Dictionary<string, int> _counts = new();
        private int _total;
        #endregion

        #region Properties
        public int Total
        {
            get { return _total; }
            private set { _total = value; }
        }

        public IReadOnlyDictionary<string, int> Counts => _counts;
        #endregion

        public Inventory()
        {
            _total = 0;
        }

        // Adds items of one kind.
        // Negative counts are refused.
        public void Add(string item, int count)
        {
            if (count < 0)
            {
                throw new ArgumentOutOfRangeException(nameof(count));
            }
            _counts[item] = _counts.TryGetValue(item, out var c) ? c + count : count;
            Total += count;
        }

        public IEnumerable<string> Missing(IEnumerable<string> wanted)
        {
            foreach (var w in wanted)
            {
                if (!_counts.ContainsKey(w))
                {
                    yield return w;
                }
            }
        }

        public string Describe() => string.Join(", ", _counts.Select(p => $"{p.Key}: {p.Value}"));
    }
}
