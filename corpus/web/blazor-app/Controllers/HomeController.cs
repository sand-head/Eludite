using Microsoft.AspNetCore.Mvc;

namespace BlazorApp.Controllers;

public class HomeController : Controller
{
    public IActionResult Index() => View();
}
