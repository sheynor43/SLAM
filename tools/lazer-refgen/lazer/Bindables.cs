// Extracted verbatim from osu-framework 2026.921.1: osu.Framework/Bindables/Bindable.cs (lines 252-303, Parse), osu.Framework/Bindables/BindableBool.cs (lines 10-25, plus a shell of the value-changed event and BindTo), osu.Framework/Bindables/RangeConstrainedBindable.cs (lines 45-49, 246, 256; MinValue and MaxValue as plain properties), osu.Framework/Bindables/BindableNumber.cs (lines 19-32 without the type validation, 69-90, 92-94, 97-110, 209; Precision without its event), in minimal class shells without binding, events and leases (except the BindableBool shell of the value-changed event and BindTo, and BindableFloat); see README.md.
// Copyright (c) ppy Pty Ltd <contact@ppy.sh>. Licensed under the MIT Licence.
// See the LICENCE file in the repository root for full licence text.

#nullable disable
#pragma warning disable CS8632 // the verbatim sources carry nullable annotations

using System;
using System.Collections.Generic;
using System.Globalization;
using System.Numerics;
using osu.Framework.Extensions.ObjectExtensions;
using osu.Framework.Extensions.TypeExtensions;

namespace osu.Framework.Bindables
{
    public class Bindable<T>
    {
        public T Default;

        private T value;

        public virtual T Value
        {
            get => value;
            set => this.value = value;
        }

        public Bindable(T defaultValue = default)
        {
            value = Default = defaultValue;
        }

        public virtual void Parse(object input, IFormatProvider provider)
        {
            switch (input)
            {
                // Of note, this covers the case when the input is a string and `T` is `string`.
                // Both `string.Empty` and `null` are valid values for this type.
                case T t:
                    Value = t;
                    break;

                case null:
                    // Nullable value types and reference types (annotated or not) are allowed to be initialised with `null`.
                    if (typeof(T).IsNullable() || typeof(T).IsClass)
                    {
                        Value = default;
                        break;
                    }

                    // Non-nullable value types can't convert from null.
                    throw new ArgumentNullException(nameof(input));

                case IBindable:
                    if (!(input is IBindable<T> bindable))
                        throw new ArgumentException($"Expected bindable of type {nameof(IBindable)}<{typeof(T)}>, got {input.GetType()}", nameof(input));

                    Value = bindable.Value;
                    break;

                default:
                    if (input is string strInput && string.IsNullOrEmpty(strInput))
                    {
                        // Nullable value types and reference types are initialised to `null` on empty strings.
                        if (typeof(T).IsNullable() || typeof(T).IsClass)
                        {
                            Value = default;
                            break;
                        }

                        // Most likely all conversion methods will not accept empty strings, but we let this fall through so that the exception is thrown by .NET itself.
                        // For example, DateTime.Parse() throws a more contextually relevant exception than int.Parse().
                    }

                    Type underlyingType = typeof(T).GetUnderlyingNullableType() ?? typeof(T);

                    if (underlyingType.IsEnum)
                        Value = (T)Enum.Parse(underlyingType, input.ToString().AsNonNull());
                    else
                        Value = (T)Convert.ChangeType(input, underlyingType, provider);

                    break;
            }
        }
    }

    public class BindableBool : Bindable<bool>
    {
        public BindableBool(bool value = false)
            : base(value)
        {
        }

        // Shell of the value-changed event and of BindTo (Bindable.cs): a changed value is
        // reported to the subscribers and copied to the bound bindables, nothing else.
        private readonly List<Action<bool>> valueChanged = new List<Action<bool>>();
        private readonly List<BindableBool> bindings = new List<BindableBool>();

        public override bool Value
        {
            get => base.Value;
            set
            {
                if (base.Value == value)
                    return;

                base.Value = value;

                foreach (var action in valueChanged)
                    action(value);

                foreach (var binding in bindings)
                    binding.Value = value;
            }
        }

        public void BindValueChanged(Action<bool> onChange) => valueChanged.Add(onChange);

        public void BindTo(BindableBool them)
        {
            Value = them.Value;
            bindings.Add(them);
            them.bindings.Add(this);
        }

        public override void Parse(object? input, IFormatProvider provider)
        {
            ArgumentNullException.ThrowIfNull(input);

            if (input is "1")
                Value = true;
            else if (input is "0")
                Value = false;
            else
                base.Parse(input, provider);
        }
    }

    public abstract class RangeConstrainedBindable<T> : Bindable<T>
    {
        private T minValue;
        private T maxValue;

        // Shells: the originals also re-clamp the current value and raise events.
        public T MinValue
        {
            get => minValue;
            set => minValue = value;
        }

        public T MaxValue
        {
            get => maxValue;
            set => maxValue = value;
        }

        protected RangeConstrainedBindable(T defaultValue = default)
            : base(defaultValue)
        {
            minValue = DefaultMinValue;
            maxValue = DefaultMaxValue;
        }

        protected abstract T DefaultMinValue { get; }

        protected abstract T DefaultMaxValue { get; }

        public override T Value
        {
            get => base.Value;
            set => setValue(value);
        }

        protected abstract T ClampValue(T value, T minValue, T maxValue);

        private void setValue(T value) => base.Value = ClampValue(value, minValue, maxValue);
    }

    public class BindableNumber<T> : RangeConstrainedBindable<T>
        where T : struct, INumber<T>, IMinMaxValue<T>
    {
        private T precision;

        public T Precision
        {
            get => precision;
            set
            {
                precision = value;
                setValue(Value);
            }
        }

        public BindableNumber(T defaultValue = default)
            : base(defaultValue)
        {
            precision = DefaultPrecision;

            // Re-apply the current value to apply the default precision value
            setValue(Value);
        }

        public override T Value
        {
            get => base.Value;
            set => setValue(value);
        }

        private void setValue(T value)
        {
            decimal decPrecision = decimal.CreateTruncating(Precision);

            if (decPrecision > 0)
            {
                // this rounding is purposefully performed on `decimal` to ensure that the resulting value is the closest possible floating-point
                // number to actual real-world base-10 decimals, as that is the most common usage of precision.
                decimal accurateResult = decimal.CreateTruncating(T.Clamp(value, MinValue, MaxValue));
                accurateResult = Math.Round(accurateResult / decPrecision) * decPrecision;

                base.Value = T.CreateTruncating(accurateResult);
            }
            else
                base.Value = value;
        }

        /// <summary>
        /// The default <see cref="Precision"/>.
        /// </summary>
        protected virtual T DefaultPrecision
        {
            get
            {
                if (typeof(T) == typeof(float))
                    return (T)(object)float.Epsilon;
                if (typeof(T) == typeof(double))
                    return (T)(object)double.Epsilon;

                return T.One;
            }
        }

        protected override T DefaultMinValue => T.MinValue;

        protected override T DefaultMaxValue => T.MaxValue;

        protected sealed override T ClampValue(T value, T minValue, T maxValue) => T.Clamp(value, minValue, maxValue);
    }
}

namespace osu.Framework.Bindables
{
    public class BindableInt : BindableNumber<int>
    {
        public BindableInt(int defaultValue = 0)
            : base(defaultValue)
        {
        }
    }

    public class BindableFloat : BindableNumber<float>
    {
        public BindableFloat(float defaultValue = 0)
            : base(defaultValue)
        {
        }
    }

    public class BindableDouble : BindableNumber<double>
    {
        public BindableDouble(double defaultValue = 0)
            : base(defaultValue)
        {
        }
    }
}

namespace osu.Framework.Extensions.ObjectExtensions
{
    public static class ObjectExtensions
    {
        public static T AsNonNull<T>(this T? obj) => obj!;
    }
}

namespace osu.Framework.Extensions.TypeExtensions
{
    public static class TypeExtensions
    {
        public static bool IsNullable(this Type type) => type.GetUnderlyingNullableType() != null;

        // The runtime cache of the original is left out.
        public static Type GetUnderlyingNullableType(this Type type)
        {
            if (!type.IsGenericType)
                return null;

            return Nullable.GetUnderlyingType(type);
        }
    }
}

namespace osu.Framework.Bindables
{
    // Stand-ins for the interfaces Parse checks against; no value here implements them.
    public interface IBindable
    {
    }

    public interface IBindable<T> : IBindable
    {
        T Value { get; }
    }
}
