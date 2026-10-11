// Extracted verbatim from osu-framework 2026.921.1: osu-framework/osu.Framework/Lists/SortedList.cs (lines 17, 19, 21, 25-29, 34-37, 52-56, 58-62, 66, 73-84, 132, 134, 136); only the members the health processors use; the class declares IEnumerable<T> only (the other collection interfaces, removal, search and serialization members are omitted). See README.md.
// Copyright (c) ppy Pty Ltd <contact@ppy.sh>. Licensed under the MIT Licence.
// See the LICENCE file in the repository root for full licence text.
using System;
using System.Collections;
using System.Collections.Generic;

namespace osu.Framework.Lists
{
    public class SortedList<T> : IEnumerable<T>
    {
        private readonly List<T> list;

        public IComparer<T> Comparer { get; }

        public int Count => list.Count;

        public T this[int index]
        {
            get => list[index];
            set => list[index] = value;
        }

        public SortedList()
            : this(Comparer<T>.Default)
        {
        }

        public SortedList(IComparer<T> comparer)
        {
            list = new List<T>();
            Comparer = comparer;
        }

        public void AddRange(IEnumerable<T> collection)
        {
            foreach (var i in collection)
                Add(i);
        }

        public virtual int Add(T value) => addInternal(value);

        private int addInternal(T value)
        {
            ArgumentNullException.ThrowIfNull(value);

            int index = list.BinarySearch(value, Comparer);
            if (index < 0)
                index = ~index;

            list.Insert(index, value);

            return index;
        }

        public List<T>.Enumerator GetEnumerator() => list.GetEnumerator();

        IEnumerator<T> IEnumerable<T>.GetEnumerator() => GetEnumerator();

        IEnumerator IEnumerable.GetEnumerator() => GetEnumerator();
    }
}
